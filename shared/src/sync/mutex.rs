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

use super::INTER_PROCESS;

#[repr(C)]
#[derive(Debug)]
pub struct Mutex<T> {
    lock: UnsafeCell<MaybeUninit<pthread_mutex_t>>,
    data: UnsafeCell<T>,
}

impl<T> Mutex<T> {
    unsafe fn init_lock(lock: *mut pthread_mutex_t) {
        let mut attr = MaybeUninit::uninit();
        pthread_mutexattr_init(attr.as_mut_ptr())
            .r("attr_init")
            .unwrap();

        pthread_mutexattr_setpshared(attr.as_mut_ptr(), INTER_PROCESS)
            .r("attr_setpshared")
            .unwrap();

        pthread_mutex_init(lock, attr.as_ptr())
            .r("mutex_init")
            .unwrap();
    }

    pub fn new(value: T) -> Self {
        let lock = UnsafeCell::new(MaybeUninit::uninit());
        let data = UnsafeCell::new(value);
        unsafe { Self::init_lock((*lock.get()).as_mut_ptr()) };
        Self { lock, data }
    }

    pub unsafe fn init_at(target: *mut Self, init_data: impl FnOnce(*mut T)) {
        let lock = &raw mut (*target).lock;
        let data = &raw mut (*target).data;
        let lock: *mut pthread_mutex_t = lock.cast();
        let data: *mut T = data.cast();
        unsafe { Self::init_lock(lock) };
        init_data(data);
    }

    pub fn lock(&self) -> MutexGuard<T> {
        unsafe {
            if pthread_mutex_lock((*self.lock.get()).as_mut_ptr()) != 0 {
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

impl<'a, T: 'a> MutexGuard<'a, T> {
    pub fn get_inner_lock(&self) -> *mut pthread_mutex_t {
        unsafe { (*self.lock.lock.get()).as_mut_ptr() }
    }
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
            if pthread_mutex_unlock((*self.lock.lock.get()).as_mut_ptr()) != 0 {
                panic!("failed to unlock mutex");
            }
        }
    }
}

unsafe impl<T> Send for Mutex<T> {}
unsafe impl<T> Sync for Mutex<T> {}

impl<T> Drop for Mutex<T> {
    fn drop(&mut self) {
        if unsafe { pthread_mutex_destroy((*self.lock.get()).as_mut_ptr()) } != 0 {
            panic!("failed to destroy mutex");
        }
    }
}

unsafe impl<T> ShmSafe for Mutex<T> where T: ShmSafe {}
