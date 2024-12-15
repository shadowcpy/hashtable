use std::{
    mem::offset_of,
    os::fd::OwnedFd,
    ptr::null_mut,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::bail;
use libc::{__errno_location, c_int, sem_getvalue, sem_t, sem_timedwait, timespec};
use macros::get_field_ptr;
use rustix::mm::{mmap, MapFlags, ProtFlags};

pub const MAGIC_VALUE: u32 = 0x77256810;
pub const SHM_REQUEST: &str = "/hashtable_req";
pub const SHM_RESPONSE: &str = "/hashtable_res";

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
    Insert(u32, u32),
    ReadBucket(u32),
    Delete(u32),
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct ResponseFrame {
    readers: sem_t,
    magic: u32,
    num_readers: u32,
    start: sem_t,
    end: sem_t,
    end_ack: sem_t,
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
    Inserted(u32),
    BucketContent { len: usize, data: [(u32, u32); 32] },
    Deleted,
    NotFound,
    Overflow,
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

        let magic: *mut u32 = get_field_ptr!(ptr, RequestFrame, magic);
        let waker: *mut sem_t = get_field_ptr!(ptr, RequestFrame, waker);
        let busy: *mut sem_t = get_field_ptr!(ptr, RequestFrame, busy);
        let data: *mut RequestData = get_field_ptr!(ptr, RequestFrame, data);

        Ok(Self {
            magic,
            waker,
            busy,
            data,
        })
    }
}

pub struct SharedResponse {
    pub magic: *mut u32,
    pub readers: *mut sem_t,
    pub num_readers: *mut u32,
    pub start: *mut sem_t,
    pub end: *mut sem_t,
    pub end_ack: *mut sem_t,
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

        let magic: *mut u32 = get_field_ptr!(ptr, ResponseFrame, magic);
        let readers: *mut sem_t = get_field_ptr!(ptr, ResponseFrame, readers);
        let num_readers: *mut u32 = get_field_ptr!(ptr, ResponseFrame, num_readers);
        let start: *mut sem_t = get_field_ptr!(ptr, ResponseFrame, start);
        let end: *mut sem_t = get_field_ptr!(ptr, ResponseFrame, end);
        let end_ack: *mut sem_t = get_field_ptr!(ptr, ResponseFrame, end_ack);
        let data: *mut ResponseData = get_field_ptr!(ptr, ResponseFrame, data);

        Ok(Self {
            magic,
            readers,
            num_readers,
            start,
            end,
            end_ack,
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

pub unsafe fn sem_wait_timeout(sem: *mut sem_t, timeout: Duration) -> c_int {
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

mod macros {
    macro_rules! get_field_ptr {
        ($ptr:expr, $frame:ty, $field:ident) => {
            unsafe { $ptr.byte_add(offset_of!($frame, $field)) }.cast()
        };
    }
    pub(crate) use get_field_ptr;
}
