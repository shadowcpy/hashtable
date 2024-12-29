use std::{
    fmt::Debug,
    mem::MaybeUninit,
    os::fd::OwnedFd,
    ptr::{copy_nonoverlapping, null_mut},
};

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
    pub fn create(
        descriptor: impl Into<String>,
        init: impl FnOnce(&mut MaybeUninit<T>),
    ) -> anyhow::Result<Self> {
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
            *contents = MaybeUninit::uninit();

            init(&mut *contents);

            *magic = MAGIC_VALUE;
        }

        Ok(Self {
            descriptor,
            memory: ptr,
            is_initiator: true,
        })
    }

    pub unsafe fn join(descriptor: impl Into<String>) -> anyhow::Result<Self> {
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

    pub fn get(&self) -> &T {
        unsafe { (*self.memory).contents.assume_init_ref() }
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

unsafe impl<T: Send> Send for SharedMemory<T> {}
unsafe impl<T: Sync> Sync for SharedMemory<T> {}

#[repr(C)]
pub struct SharedMemoryContents<T> {
    magic: u32,
    contents: MaybeUninit<T>,
}

#[derive(Debug)]
pub struct HeapArrayInit<T, const N: usize> {
    inner: Vec<T>,
}

impl<T: Debug, const N: usize> HeapArrayInit<T, N> {
    pub fn from_fn(mut init: impl FnMut(usize) -> T) -> Self {
        let mut vec = Vec::with_capacity(N);
        for index in 0..N {
            vec.push(init(index));
        }
        Self { inner: vec }
    }

    pub unsafe fn move_to(self, target: *mut [T; N]) {
        let slice = self.inner.into_boxed_slice();
        let array: Box<[T; N]> = slice.try_into().unwrap();
        let array = Box::into_raw(array);
        copy_nonoverlapping(array, target, 1);
    }
}
