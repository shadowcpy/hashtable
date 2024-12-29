use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use libc::{
    __errno_location, c_int, pthread_cond_broadcast, pthread_cond_destroy, pthread_cond_init,
    pthread_cond_signal, pthread_cond_t, pthread_cond_timedwait, pthread_cond_wait,
    pthread_condattr_init, pthread_condattr_setpshared, pthread_mutex_t, timespec, ETIMEDOUT,
};

use crate::{shm::ShmSafe, CheckOk};

use super::MutexGuard;

#[repr(C)]
#[derive(Debug)]
pub struct Condvar {
    inner: UnsafeCell<MaybeUninit<pthread_cond_t>>,
}

impl Condvar {
    pub fn new(inter_process: bool) -> Self {
        let inner = UnsafeCell::new(MaybeUninit::uninit());
        let mut attr = MaybeUninit::uninit();
        unsafe {
            pthread_condattr_init(attr.as_mut_ptr())
                .r("attr_init")
                .unwrap();

            if inter_process {
                pthread_condattr_setpshared(attr.as_mut_ptr(), 1)
                    .r("attr_setpshared")
                    .unwrap();
            }

            pthread_cond_init((*inner.get()).as_mut_ptr(), attr.as_ptr())
                .r("cond_init")
                .unwrap();
        }

        Self { inner }
    }

    pub fn signal(&self) {
        unsafe {
            if pthread_cond_signal((*self.inner.get()).as_mut_ptr()) != 0 {
                panic!("failed to signal condvar");
            }
        }
    }

    pub fn broadcast(&self) {
        unsafe {
            if pthread_cond_broadcast((*self.inner.get()).as_mut_ptr()) != 0 {
                panic!("failed to broadcast condvar");
            }
        }
    }

    pub fn wait<'m, T>(&self, guard: MutexGuard<'m, T>) -> MutexGuard<'m, T> {
        unsafe {
            if pthread_cond_wait((*self.inner.get()).as_mut_ptr(), guard.get_inner_lock()) != 0 {
                panic!("failed to wait on condvar");
            }
        }
        guard
    }

    pub fn wait_timeout<'m, T>(
        &self,
        guard: MutexGuard<'m, T>,
        timeout: Duration,
    ) -> Option<MutexGuard<'m, T>> {
        let result = unsafe {
            cond_wait_timeout(
                (*self.inner.get()).as_mut_ptr(),
                guard.get_inner_lock(),
                timeout,
            )
        };
        match result {
            0 => Some(guard),
            ETIMEDOUT => None,
            e => panic!("failed to wait for condvar: {e}"),
        }
    }
}

unsafe impl Send for Condvar {}
unsafe impl Sync for Condvar {}

impl Drop for Condvar {
    fn drop(&mut self) {
        if unsafe { pthread_cond_destroy((*self.inner.get()).as_mut_ptr()) } != 0 {
            panic!("failed to destroy condvar");
        }
    }
}

unsafe impl ShmSafe for Condvar {}

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
