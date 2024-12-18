use std::{
    mem::offset_of,
    os::fd::OwnedFd,
    ptr::null_mut,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::bail;
use arrayvec::ArrayString;
use libc::{__errno_location, c_int, sem_getvalue, sem_t, sem_timedwait, sem_trywait, timespec};
use rustix::mm::{mmap, MapFlags, ProtFlags};

use macros::get_field_ptr;

pub const MAGIC_VALUE: u32 = 0x77256810;
pub const SHM_REQUEST: &str = "/hashtable_req";
pub const SHM_RESPONSE: &str = "/hashtable_res";

pub type KeyType = ArrayString<64>;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct RequestFrame {
    magic: u32,
    waker: sem_t,
    busy: sem_t,
    data: RequestData,
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

pub struct SharedRequest {
    pub magic: *mut u32,
    pub waker: *mut sem_t,
    pub busy: *mut sem_t,
    pub data: *mut RequestData,
}

unsafe impl Send for SharedRequest {}

impl SharedRequest {
    pub fn from_fd(fd: OwnedFd) -> anyhow::Result<Self> {
        let ptr = unsafe {
            // Safety: Ptr is null
            mmap(
                null_mut(),
                size_of::<RequestFrame>(),
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &fd,
                0,
            )?
        };

        let magic: *mut u32 = get_field_ptr!(magic, RequestFrame, ptr);
        let waker: *mut sem_t = get_field_ptr!(waker, RequestFrame, ptr);
        let busy: *mut sem_t = get_field_ptr!(busy, RequestFrame, ptr);
        let data: *mut RequestData = get_field_ptr!(data, RequestFrame, ptr);

        Ok(Self {
            magic,
            waker,
            busy,
            data,
        })
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct ResponseFrame {
    magic: u32,
    readers: sem_t,
    num_readers: u32,
    barrier1: sem_t,
    barrier2: sem_t,
    read_complete: sem_t,
    write_complete: sem_t,
    count_mutex: sem_t,
    count: u32,
    data: ResponseData,
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

pub struct SharedResponse {
    pub magic: *mut u32,
    pub readers: *mut sem_t,
    pub num_readers: *mut u32,
    pub barrier1: *mut sem_t,
    pub barrier2: *mut sem_t,
    pub read_complete: *mut sem_t,
    pub write_complete: *mut sem_t,
    pub count_mutex: *mut sem_t,
    pub count: *mut u32,
    pub data: *mut ResponseData,
}

unsafe impl Send for SharedResponse {}

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
        let readers: *mut sem_t = get_field_ptr!(readers, ResponseFrame, ptr);
        let num_readers: *mut u32 = get_field_ptr!(num_readers, ResponseFrame, ptr);
        let barrier1: *mut sem_t = get_field_ptr!(barrier1, ResponseFrame, ptr);
        let barrier2: *mut sem_t = get_field_ptr!(barrier2, ResponseFrame, ptr);
        let read_complete: *mut sem_t = get_field_ptr!(read_complete, ResponseFrame, ptr);
        let write_complete: *mut sem_t = get_field_ptr!(write_complete, ResponseFrame, ptr);
        let count_mutex: *mut sem_t = get_field_ptr!(count_mutex, ResponseFrame, ptr);
        let count: *mut u32 = get_field_ptr!(count, ResponseFrame, ptr);
        let data: *mut ResponseData = get_field_ptr!(data, ResponseFrame, ptr);

        Ok(Self {
            magic,
            readers,
            num_readers,
            barrier1,
            barrier2,
            read_complete,
            write_complete,
            count_mutex,
            count,
            data,
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
