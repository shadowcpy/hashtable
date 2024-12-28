use std::{os::fd::OwnedFd, ptr::null_mut};

use anyhow::{bail, Context};
use rustix::{
    fs::{ftruncate, Mode},
    mm::{mmap, munmap, MapFlags, ProtFlags},
    shm::{self, OFlags},
};

use crate::MAGIC_VALUE;

pub unsafe trait ShmSafe {}

pub struct SharedMemory<T> {
    is_initiator: bool,
    descriptor: String,
    memory: *mut SharedMemoryContents<T>,
}

impl<T: ShmSafe> SharedMemory<T> {
    pub fn create(descriptor: impl Into<String>, value: T) -> anyhow::Result<Self> {
        let descriptor = descriptor.into();

        let _ = shm::unlink(&descriptor);

        let fd = shm::open(
            &descriptor,
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR,
            Mode::RUSR | Mode::WUSR,
        )?;

        ftruncate(&fd, size_of::<SharedMemoryContents<T>>() as u64)?;
        let ptr = unsafe { Self::mmap(fd)? };

        unsafe {
            let magic = &raw mut (*ptr).magic;
            let contents = &raw mut (*ptr).contents;

            *contents = value;
            *magic = MAGIC_VALUE;
        }

        Ok(Self {
            descriptor,
            memory: ptr,
            is_initiator: true,
        })
    }

    pub fn join(descriptor: impl Into<String>) -> anyhow::Result<Self> {
        let descriptor = descriptor.into();
        let fd = shm::open(&descriptor, OFlags::RDWR, Mode::RUSR | Mode::WUSR)
            .context("Opening shared memory failed")?;

        let ptr = unsafe { Self::mmap(fd)? };

        unsafe {
            let magic = &raw mut (*ptr).magic;

            if *magic != MAGIC_VALUE {
                bail!("Memory not ready yet");
            }
        }

        Ok(Self {
            descriptor,
            memory: ptr,
            is_initiator: false,
        })
    }

    pub fn get(&self) -> *mut T {
        unsafe { &raw mut (*self.memory).contents }
    }

    unsafe fn mmap(fd: OwnedFd) -> anyhow::Result<*mut SharedMemoryContents<T>> {
        // Safety: Ptr is null
        Ok(mmap(
            null_mut(),
            size_of::<SharedMemoryContents<T>>(),
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::SHARED,
            &fd,
            0,
        )?
        .cast())
    }
}

impl<T> Drop for SharedMemory<T> {
    fn drop(&mut self) {
        if self.is_initiator {
            let _ = shm::unlink(&self.descriptor);
        }
        unsafe {
            let _ = munmap(self.memory.cast(), size_of::<SharedMemoryContents<T>>());
        }
    }
}

#[repr(C)]
pub struct SharedMemoryContents<T> {
    magic: u32,
    contents: T,
}
