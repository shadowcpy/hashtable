mod condvar;
mod mutex;
mod rwlock;
mod semaphore;

const INTER_PROCESS: i32 = 1;

pub use condvar::*;
pub use mutex::*;
pub use rwlock::*;
pub use semaphore::*;
