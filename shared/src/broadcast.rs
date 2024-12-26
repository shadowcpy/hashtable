use std::{cell::UnsafeCell, mem::MaybeUninit, sync::atomic::AtomicUsize};

use libc::sem_t;

#[repr(C)]
struct Broadcast<T, const N: usize> {
    buffer: [Slot<T>; N],
    tail_lock: sem_t,
    tail_pos: u64,
    tail_rx_cnt: usize,
}

#[repr(C)]
struct Slot<T> {
    lock: sem_t,
    rem: AtomicUsize,
    val: UnsafeCell<MaybeUninit<T>>,
}

struct Receiver<T, const N: usize> {
    shared: *mut Broadcast<T, N>,
    next: u64,
}

struct Sender<T, const N: usize> {
    shared: *mut Broadcast<T, N>,
}
