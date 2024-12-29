use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
};

use libc::{
    pthread_mutex_destroy, pthread_mutex_init, pthread_mutex_lock, pthread_mutex_t,
    pthread_mutex_unlock, pthread_mutexattr_init, pthread_mutexattr_setpshared,
};

use crate::{shm::ShmSafe, CheckOk};

#[repr(C)]
#[derive(Debug)]
pub struct Mutex<T> {
    inner: UnsafeCell<MaybeUninit<pthread_mutex_t>>,
    data: UnsafeCell<T>,
}

impl<T> Mutex<T> {
    pub fn new(data: T, inter_process: bool) -> Self {
        let inner = UnsafeCell::new(MaybeUninit::uninit());
        let mut attr = MaybeUninit::uninit();
        unsafe {
            pthread_mutexattr_init(attr.as_mut_ptr())
                .r("attr_init")
                .unwrap();

            if inter_process {
                pthread_mutexattr_setpshared(attr.as_mut_ptr(), 1)
                    .r("attr_setpshared")
                    .unwrap();
            }

            pthread_mutex_init((*inner.get()).as_mut_ptr(), attr.as_ptr())
                .r("mutex_init")
                .unwrap();
        }

        Self {
            inner,
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock(&self) -> MutexGuard<T> {
        unsafe {
            if pthread_mutex_lock((*self.inner.get()).as_mut_ptr()) != 0 {
                panic!("failed to lock mutex");
            }
            MutexGuard {
                lock: self,
                data: &mut *self.data.get(),
            }
        }
    }
}

pub struct MutexGuard<'a, T: 'a> {
    lock: &'a Mutex<T>,
    data: &'a mut T,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.data
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        unsafe {
            if pthread_mutex_unlock((*self.lock.inner.get()).as_mut_ptr()) != 0 {
                panic!("failed to unlock mutex");
            }
        }
    }
}

unsafe impl<T> Send for Mutex<T> {}
unsafe impl<T> Sync for Mutex<T> {}

impl<T> Drop for Mutex<T> {
    fn drop(&mut self) {
        if unsafe { pthread_mutex_destroy((*self.inner.get()).as_mut_ptr()) } != 0 {
            panic!("failed to destroy mutex");
        }
    }
}

unsafe impl<T> ShmSafe for Mutex<T> where T: ShmSafe {}
