use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
};

use libc::{
    pthread_rwlock_destroy, pthread_rwlock_init, pthread_rwlock_rdlock, pthread_rwlock_t,
    pthread_rwlock_unlock, pthread_rwlock_wrlock, pthread_rwlockattr_init,
    pthread_rwlockattr_setpshared,
};

use crate::{shm::ShmSafe, CheckOk};

#[repr(C)]
#[derive(Debug)]
pub struct RwLock<T> {
    inner: UnsafeCell<MaybeUninit<pthread_rwlock_t>>,
    data: UnsafeCell<T>,
}

impl<T> RwLock<T> {
    pub fn new(data: T, inter_process: bool) -> Self {
        let inner = UnsafeCell::new(MaybeUninit::uninit());
        let mut attr = MaybeUninit::uninit();
        unsafe {
            pthread_rwlockattr_init(attr.as_mut_ptr())
                .r("attr_init")
                .unwrap();

            if inter_process {
                pthread_rwlockattr_setpshared(attr.as_mut_ptr(), 1)
                    .r("attr_setpshared")
                    .unwrap();
            }

            pthread_rwlock_init((*inner.get()).as_mut_ptr(), attr.as_ptr())
                .r("rwlock_init")
                .unwrap();
        }

        Self {
            inner,
            data: UnsafeCell::new(data),
        }
    }

    pub fn read(&self) -> RwLockReadGuard<T> {
        unsafe {
            if pthread_rwlock_rdlock((*self.inner.get()).as_mut_ptr()) != 0 {
                panic!("failed to wait for semaphore");
            }
            RwLockReadGuard {
                lock: self,
                data: &*self.data.get(),
            }
        }
    }

    pub fn write(&self) -> RwLockWriteGuard<T> {
        unsafe {
            if pthread_rwlock_wrlock((*self.inner.get()).as_mut_ptr()) != 0 {
                panic!("failed to wait for semaphore");
            }
            RwLockWriteGuard {
                lock: self,
                data: &mut *self.data.get(),
            }
        }
    }

    pub unsafe fn bypass(&self) -> &mut T {
        unsafe { &mut *self.data.get() }
    }
}

pub struct RwLockReadGuard<'a, T: 'a> {
    lock: &'a RwLock<T>,
    data: &'a T,
}

impl<T> Deref for RwLockReadGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

impl<T> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        unsafe {
            if pthread_rwlock_unlock((*self.lock.inner.get()).as_mut_ptr()) != 0 {
                panic!("failed to unlock rwlock");
            }
        }
    }
}

pub struct RwLockWriteGuard<'a, T: 'a> {
    lock: &'a RwLock<T>,
    data: &'a mut T,
}

impl<T> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

impl<T> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.data
    }
}

impl<T> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        unsafe {
            if pthread_rwlock_unlock((*self.lock.inner.get()).as_mut_ptr()) != 0 {
                panic!("failed to unlock rwlock");
            }
        }
    }
}

unsafe impl<T> Send for RwLock<T> {}
unsafe impl<T> Sync for RwLock<T> {}

impl<T> Drop for RwLock<T> {
    fn drop(&mut self) {
        if unsafe { pthread_rwlock_destroy((*self.inner.get()).as_mut_ptr()) } != 0 {
            panic!("failed to destroy rwlock");
        }
    }
}

unsafe impl<T> ShmSafe for RwLock<T> where T: ShmSafe {}
