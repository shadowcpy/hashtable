use std::{mem::MaybeUninit, ptr, sync::atomic::AtomicUsize};

use anyhow::bail;
use arrayvec::ArrayString;
use libc::c_int;
use sync::{Mutex, RwLock, Semaphore};

use shm::{HeapArrayInit, ShmSafe};

pub mod shm;
pub mod sync;

pub const MAGIC_VALUE: u32 = 0x77256810;
pub const DESCRIPTOR: &str = "/hashtable";

pub const REQ_BUFFER_SIZE: usize = 2048;
pub const RES_BUFFER_SIZE: usize = 2048;

pub type KeyType = ArrayString<64>;
pub type ValueType = u32;

#[repr(C)]
#[derive(Debug)]
pub struct HashtableMemory {
    pub request_frame: RequestFrame,
    pub response_frame: ResponseFrame,
}

unsafe impl ShmSafe for HashtableMemory {}

impl HashtableMemory {
    /// Use a custom, unsafe initializer. This is required because
    /// the ring buffers (arrays) can overflow the stack on construction
    /// (before being able to move them to shared memory)
    pub unsafe fn init_in_shm(shm: *mut HashtableMemory, num_writers: usize) {
        // Initialize Request Frame
        {
            let count = &raw mut (*shm).request_frame.count;
            let space = &raw mut (*shm).request_frame.space;
            let queue = &raw mut (*shm).request_frame.queue;

            ptr::write(count, Semaphore::new(0));
            ptr::write(space, Semaphore::new(REQ_BUFFER_SIZE as u32));
            Mutex::init_at(queue, |queue_inner| {
                let write = &raw mut (*queue_inner).write;
                let read = &raw mut (*queue_inner).read;
                let buffer = &raw mut (*queue_inner).buffer;

                ptr::write(write, 0);
                ptr::write(read, 0);

                // The relevant part: initialize the array on the heap
                // and move it to shared memory
                let init_buffer = HeapArrayInit::from_fn(|_| MaybeUninit::uninit());
                init_buffer.move_to(buffer);
            });
        }

        // Initialize Response Frame
        {
            let buffer = &raw mut (*shm).response_frame.buffer;
            let space = &raw mut (*shm).response_frame.space;
            let num_tx = &raw mut (*shm).response_frame.num_tx;
            let tail = &raw mut (*shm).response_frame.tail;

            let init_buffer = HeapArrayInit::from_fn(|index| {
                RwLock::new(ResponseSlot {
                    rem: AtomicUsize::new(0),
                    pos: (index as u64).wrapping_sub(RES_BUFFER_SIZE as u64),
                    val: MaybeUninit::uninit(),
                })
            });

            init_buffer.move_to(buffer);

            ptr::write(space, Semaphore::new(RES_BUFFER_SIZE as u32));
            ptr::write(num_tx, num_writers);
            ptr::write(tail, Mutex::new(ResponseTail { pos: 0, rx_cnt: 0 }));
        }
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct RequestFrame {
    pub count: Semaphore,
    pub space: Semaphore,
    pub queue: Mutex<RequestQueue>,
}

#[repr(C)]
#[derive(Debug)]
pub struct RequestQueue {
    pub write: usize,
    pub read: usize,
    pub buffer: [MaybeUninit<RequestData>; REQ_BUFFER_SIZE],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct RequestData {
    pub client_id: u32,
    pub request_id: u32,
    pub payload: RequestPayload,
}

#[repr(C, u8)]
#[derive(Debug, Copy, Clone)]
pub enum RequestPayload {
    Insert(KeyType, ValueType),
    ReadBucket(KeyType),
    PrintHashmap,
    Delete(KeyType),
}

#[repr(C)]
#[derive(Debug)]
pub struct ResponseFrame {
    pub buffer: [RwLock<ResponseSlot>; RES_BUFFER_SIZE],
    pub space: Semaphore,
    pub num_tx: usize,
    pub tail: Mutex<ResponseTail>,
}

#[repr(C)]
#[derive(Debug)]
pub struct ResponseTail {
    pub pos: u64,
    pub rx_cnt: usize,
}

#[repr(C)]
#[derive(Debug)]
pub struct ResponseSlot {
    pub rem: AtomicUsize,
    pub pos: u64,
    pub val: MaybeUninit<ResponseData>,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct ResponseData {
    pub client_id: u32,
    pub request_id: u32,
    pub payload: ResponsePayload,
}

#[repr(C, u8)]
#[derive(Debug, Copy, Clone)]
pub enum ResponsePayload {
    Inserted,
    BucketContent {
        len: usize,
        data: [(KeyType, ValueType); 32],
    },
    Deleted,
    NotFound,
    Printed,
    Overflow,
}

pub trait CheckOk<R> {
    fn r(self, op: &str) -> Result<R, anyhow::Error>;
}

impl CheckOk<()> for c_int {
    fn r(self, op: &str) -> Result<(), anyhow::Error> {
        if self != 0 {
            bail!("Operation {op} failed: Code {self}");
        }
        Ok(())
    }
}

impl<T> CheckOk<T> for Result<T, c_int> {
    fn r(self, op: &str) -> Result<T, anyhow::Error> {
        match self {
            Ok(t) => Ok(t),
            Err(i) => bail!("Operation {op} failed: Code {i}"),
        }
    }
}
