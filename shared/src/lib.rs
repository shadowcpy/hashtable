use std::{
    mem::{offset_of, MaybeUninit},
    os::fd::OwnedFd,
    ptr::null_mut,
    sync::atomic::AtomicUsize,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::bail;
use arrayvec::ArrayString;
use libc::{
    __errno_location, c_int, pthread_cond_t, pthread_cond_timedwait, pthread_mutex_t,
    pthread_rwlock_t, sem_getvalue, sem_t, sem_timedwait, sem_trywait, timespec,
};
use rustix::mm::{mmap, MapFlags, ProtFlags};

use macros::get_field_ptr;

pub const MAGIC_VALUE: u32 = 0x77256810;
pub const SHM_REQUEST: &str = "/hashtable_req";
pub const SHM_RESPONSE: &str = "/hashtable_res";

pub const REQ_BUFFER_SIZE: usize = 1024;
pub const RES_BUFFER_SIZE: usize = 1024;

pub type KeyType = ArrayString<64>;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct RequestFrame {
    magic: u32,
    write: usize,
    read: usize,
    count: sem_t,
    space: sem_t,
    lock: sem_t,
    buffer: [MaybeUninit<RequestData>; REQ_BUFFER_SIZE],
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

#[derive(Copy, Clone)]
pub struct SharedRequest {
    pub magic: *mut u32,
    pub write: *mut usize,
    pub read: *mut usize,
    pub count: *mut sem_t,
    pub space: *mut sem_t,
    pub lock: *mut sem_t,
    pub buffer: *mut [MaybeUninit<RequestData>; REQ_BUFFER_SIZE],
}

unsafe impl Send for SharedRequest {}
unsafe impl Sync for SharedRequest {}

impl SharedRequest {
    pub fn from_fd(fd: OwnedFd) -> anyhow::Result<Self> {
        let ptr: *mut RequestFrame = unsafe {
            // Safety: Ptr is null
            mmap(
                null_mut(),
                size_of::<RequestFrame>(),
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &fd,
                0,
            )?
            .cast()
        };

        let magic: *mut u32 = get_field_ptr!(magic, RequestFrame, ptr);
        let write: *mut usize = get_field_ptr!(write, RequestFrame, ptr);
        let read: *mut usize = get_field_ptr!(read, RequestFrame, ptr);
        let count: *mut sem_t = get_field_ptr!(count, RequestFrame, ptr);
        let space: *mut sem_t = get_field_ptr!(space, RequestFrame, ptr);
        let lock: *mut sem_t = get_field_ptr!(lock, RequestFrame, ptr);
        let buffer: *mut [MaybeUninit<RequestData>; REQ_BUFFER_SIZE] =
            get_field_ptr!(buffer, RequestFrame, ptr);

        Ok(Self {
            magic,
            write,
            read,
            count,
            space,
            lock,
            buffer,
        })
    }
}

#[repr(C)]
pub struct ResponseFrame {
    magic: u32,
    buffer: [MaybeUninit<ResponseSlot>; RES_BUFFER_SIZE],
    num_tx: usize,
    tail_lock: pthread_mutex_t,
    tail_pos: u64,
    tail_rx_cnt: usize,
}

#[repr(C)]
pub struct ResponseSlot {
    pub lock: pthread_rwlock_t,
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

#[derive(Copy, Clone)]
pub struct SharedResponse {
    pub magic: *mut u32,
    pub buffer: *mut [MaybeUninit<ResponseSlot>; RES_BUFFER_SIZE],
    pub num_tx: *mut usize,
    pub tail_lock: *mut pthread_mutex_t,
    pub tail_pos: *mut u64,
    pub tail_rx_cnt: *mut usize,
}

unsafe impl Send for SharedResponse {}
unsafe impl Sync for SharedResponse {}

impl SharedResponse {
    pub fn from_fd(fd: OwnedFd) -> anyhow::Result<Self> {
        let ptr = unsafe {
            // Safety: Ptr is null
            mmap(
                null_mut(),
                size_of::<ResponseFrame>(),
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &fd,
                0,
            )?
        };

        let magic: *mut u32 = get_field_ptr!(magic, ResponseFrame, ptr);
        let buffer: *mut [MaybeUninit<ResponseSlot>; RES_BUFFER_SIZE] =
            get_field_ptr!(buffer, ResponseFrame, ptr);
        let num_tx: *mut usize = get_field_ptr!(num_tx, ResponseFrame, ptr);
        let tail_lock: *mut pthread_mutex_t = get_field_ptr!(tail_lock, ResponseFrame, ptr);
        let tail_pos: *mut u64 = get_field_ptr!(tail_pos, ResponseFrame, ptr);
        let tail_rx_cnt: *mut usize = get_field_ptr!(tail_rx_cnt, ResponseFrame, ptr);

        Ok(Self {
            magic,
            buffer,
            num_tx,
            tail_lock,
            tail_pos,
            tail_rx_cnt,
        })
    }
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

mod macros {
    macro_rules! get_field_ptr {
        ($field:ident, $frame:ty, $ptr:expr) => {
            unsafe { $ptr.byte_add(offset_of!($frame, $field)) }.cast()
        };
    }
    pub(crate) use get_field_ptr;
}
