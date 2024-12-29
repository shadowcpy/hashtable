use std::{
    mem::MaybeUninit,
    ptr,
    sync::atomic::AtomicUsize,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::bail;
use arrayvec::ArrayString;
use libc::{
    __errno_location, c_int, pthread_cond_t, pthread_cond_timedwait, pthread_mutex_t, sem_getvalue,
    sem_t, sem_timedwait, sem_trywait, timespec,
};
use sync::{Mutex, RwLock, Semaphore};

use shm::{HeapArrayInit, ShmSafe};

pub mod shm;
pub mod sync;

pub const MAGIC_VALUE: u32 = 0x77256810;
pub const DESCRIPTOR: &str = "/hashtable";

pub const REQ_BUFFER_SIZE: usize = 1024;
pub const RES_BUFFER_SIZE: usize = 1024;

pub type KeyType = ArrayString<64>;

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

            ptr::write(count, Semaphore::new(0, true));
            ptr::write(space, Semaphore::new(REQ_BUFFER_SIZE as u32, true));
            ptr::write(
                queue,
                Mutex::construct_unchecked(
                    |queue_inner| {
                        let queue_inner: *mut RequestQueue = queue_inner.as_mut_ptr();

                        let write = &raw mut (*queue_inner).write;
                        let read = &raw mut (*queue_inner).read;
                        let buffer = &raw mut (*queue_inner).buffer;

                        ptr::write(write, 0);
                        ptr::write(read, 0);

                        // The relevant part: initialize the array on the heap
                        // and move it to shared memory
                        let init_buffer = HeapArrayInit::from_fn(|_| MaybeUninit::uninit());
                        init_buffer.move_to(buffer);
                    },
                    true,
                ),
            );
        }

        // Initialize Response Frame
        {
            let buffer = &raw mut (*shm).response_frame.buffer;
            let num_tx = &raw mut (*shm).response_frame.num_tx;
            let tail = &raw mut (*shm).response_frame.tail;

            let init_buffer = HeapArrayInit::from_fn(|index| {
                RwLock::new(
                    ResponseSlot {
                        rem: AtomicUsize::new(0),
                        pos: (index as u64).wrapping_sub(RES_BUFFER_SIZE as u64),
                        val: MaybeUninit::uninit(),
                    },
                    true,
                )
            });

            init_buffer.move_to(buffer);

            ptr::write(num_tx, num_writers);
            ptr::write(tail, Mutex::new(ResponseTail { pos: 0, rx_cnt: 0 }, true));
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
    Insert(KeyType, u32),
    ReadBucket(KeyType),
    Delete(KeyType),
}

#[repr(C)]
#[derive(Debug)]
pub struct ResponseFrame {
    pub buffer: [RwLock<ResponseSlot>; RES_BUFFER_SIZE],
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
        data: [(KeyType, u32); 32],
    },
    Deleted,
    NotFound,
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

pub unsafe fn sema_getvalue(sem: *mut sem_t) -> Result<c_int, c_int> {
    let mut i: i32 = 0;
    let ptr = &raw mut i;

    let res = sem_getvalue(sem, ptr);
    if res == 0 {
        Ok(i)
    } else {
        Err(res)
    }
}

pub unsafe fn sema_wait_timeout(sem: *mut sem_t, timeout: Duration) -> c_int {
    let target = SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + timeout;
    let ts = timespec {
        tv_sec: target.as_secs() as i64,
        tv_nsec: target.subsec_nanos() as i64,
    };
    if sem_timedwait(sem, &raw const ts) != 0 {
        *__errno_location()
    } else {
        0
    }
}

pub unsafe fn cond_wait_timeout(
    cond: *mut pthread_cond_t,
    mutex: *mut pthread_mutex_t,
    timeout: Duration,
) -> c_int {
    let target = SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + timeout;
    let ts = timespec {
        tv_sec: target.as_secs() as i64,
        tv_nsec: target.subsec_nanos() as i64,
    };
    if pthread_cond_timedwait(cond, mutex, &raw const ts) != 0 {
        *__errno_location()
    } else {
        0
    }
}

pub unsafe fn sema_trywait(sem: *mut sem_t) -> c_int {
    if sem_trywait(sem) != 0 {
        *__errno_location()
    } else {
        0
    }
}
