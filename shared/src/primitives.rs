use std::{cell::UnsafeCell, mem::MaybeUninit};

use libc::{sem_destroy, sem_init, sem_post, sem_t, sem_wait};

use crate::shm::ShmSafe;

#[repr(C)]
#[derive(Debug)]
pub struct Semaphore {
    inner: UnsafeCell<MaybeUninit<sem_t>>,
}

impl Semaphore {
    pub fn new(value: u32, inter_process: bool) -> Self {
        let inter_process = if inter_process { 1 } else { 0 };
        let inner = UnsafeCell::new(MaybeUninit::uninit());
        if unsafe { sem_init((*inner.get()).as_mut_ptr(), inter_process, value) } != 0 {
            panic!("failed to initialize semaphore");
        }
        Self { inner }
    }

    pub fn wait(&self) {
        if unsafe { sem_wait((*self.inner.get()).as_mut_ptr()) } != 0 {
            panic!("failed to wait for semaphore");
        }
    }

    pub fn post(&self) {
        if unsafe { sem_post((*self.inner.get()).as_mut_ptr()) } != 0 {
            panic!("failed to post semaphore");
        }
    }
}

unsafe impl Send for Semaphore {}
unsafe impl Sync for Semaphore {}

impl Drop for Semaphore {
    fn drop(&mut self) {
        if unsafe { sem_destroy((*self.inner.get()).as_mut_ptr()) } != 0 {
            panic!("failed to destroy semaphore");
        }
    }
}

unsafe impl ShmSafe for Semaphore {}
