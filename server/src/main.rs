use std::{
    mem::MaybeUninit,
    process::exit,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

use clap::Parser;
use libc::{
    pthread_mutex_init, pthread_mutex_lock, pthread_mutex_unlock, pthread_mutexattr_init,
    pthread_mutexattr_setpshared, pthread_rwlock_init, pthread_rwlock_unlock,
    pthread_rwlock_wrlock, pthread_rwlockattr_init, pthread_rwlockattr_setpshared, sem_init,
    sem_post, sem_wait,
};
use rustix::{
    fs::{ftruncate, Mode},
    shm::{self, OFlags},
};

pub mod cli;
pub mod hash_table;

use cli::Args;
use hash_table::HashTable;
use shared::{
    primitives::Semaphore, CheckOk, KeyType, RequestFrame, RequestPayload, ResponseData,
    ResponseFrame, ResponsePayload, SharedRequest, SharedResponse, MAGIC_VALUE, REQ_BUFFER_SIZE,
    RES_BUFFER_SIZE, SHM_REQUEST, SHM_RESPONSE,
};

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let _ = shm::unlink(SHM_REQUEST);
    let _ = shm::unlink(SHM_RESPONSE);

    let hm: HashTable<KeyType, u32> = HashTable::new(args.size);

    let req_fd = shm::open(
        SHM_REQUEST,
        OFlags::CREATE | OFlags::EXCL | OFlags::RDWR,
        Mode::RUSR | Mode::WUSR,
    )?;

    let res_fd = shm::open(
        SHM_RESPONSE,
        OFlags::CREATE | OFlags::EXCL | OFlags::RDWR,
        Mode::RUSR | Mode::WUSR,
    )?;

    ftruncate(&req_fd, size_of::<RequestFrame>() as u64)?;
    ftruncate(&res_fd, size_of::<ResponseFrame>() as u64)?;

    let is = SharedRequest::from_fd(req_fd)?;
    let os = SharedResponse::from_fd(res_fd)?;

    // Requests (input)
    unsafe {
        sem_init(is.count, 1, 0).r("init_count")?;
        let space = Semaphore::new(REQ_BUFFER_SIZE as u32, true);
        *is.space = space;
        sem_init(is.lock, 1, 1).r("init_lock")?;
    }

    // Responses (output)
    unsafe {
        let mut attr = MaybeUninit::uninit();
        pthread_mutexattr_init(attr.as_mut_ptr()).r("attr_init")?;
        pthread_mutexattr_setpshared(attr.as_mut_ptr(), 1).r("attr_setpshared")?;

        pthread_mutex_init(os.tail_lock, attr.as_ptr()).r("taillock_init")?;
    }

    unsafe {
        (*is.buffer) = [MaybeUninit::uninit(); 1024];
        (*is.read) = 0;
        (*is.write) = 0;

        (*os.num_tx) = args.num_threads;
        (*os.tail_pos) = 0;
        (*os.tail_rx_cnt) = 0;
        (*os.buffer) = [const { MaybeUninit::uninit() }; RES_BUFFER_SIZE];

        let mut index = 0;
        for slot in (*os.buffer).iter_mut() {
            let slot = slot.as_mut_ptr();
            (*slot).pos = (index as u64).wrapping_sub(RES_BUFFER_SIZE as u64);
            (*slot).rem = AtomicUsize::new(0);
            (*slot).val = MaybeUninit::uninit();

            let slot_lock = &raw mut (*slot).lock;

            let mut attr = MaybeUninit::uninit();
            pthread_rwlockattr_init(attr.as_mut_ptr()).r("attr_init")?;
            pthread_rwlockattr_setpshared(attr.as_mut_ptr(), 1).r("attr_setpshared")?;

            pthread_rwlock_init(slot_lock, attr.as_ptr()).r("slot_init")?;

            index += 1;
        }

        (*is.magic) = MAGIC_VALUE;
        (*os.magic) = MAGIC_VALUE;
    }

    println!("Initialized [{} -> {}]", SHM_REQUEST, SHM_RESPONSE);

    ctrlc::set_handler(move || {
        shm::unlink(SHM_REQUEST).unwrap();
        shm::unlink(SHM_RESPONSE).unwrap();
        exit(0);
    })?;

    println!("Server is ready to accept connections");

    thread::scope(|s| {
        for i in 0..args.num_threads {
            let _worker = format!("{i}");
            s.spawn(|| {
                let mut is = is;
                let mut os = os;
                loop {
                    if let Ok(request) = is_pop_item(&mut is) {
                        let payload = match request.payload {
                            RequestPayload::Insert(k, v) => {
                                hm.insert(k, v);
                                ResponsePayload::Inserted
                            }
                            RequestPayload::ReadBucket(k) => {
                                let res = hm.read_bucket(k);
                                let list: Vec<(KeyType, u32)> =
                                    res.iter().map(|n| (n.k, n.v)).collect();
                                let len = list.len();
                                if len > 32 {
                                    ResponsePayload::Overflow
                                } else {
                                    let mut data = [(KeyType::new(), 0); 32];
                                    data[..len].copy_from_slice(&list);
                                    ResponsePayload::BucketContent { len, data }
                                }
                            }
                            RequestPayload::Delete(k) => {
                                if let Some(_v) = hm.remove(k) {
                                    ResponsePayload::Deleted
                                } else {
                                    ResponsePayload::NotFound
                                }
                            }
                        };

                        let response = ResponseData {
                            client_id: request.client_id,
                            request_id: request.request_id,
                            payload,
                        };

                        while !os_push_item(response, &mut os).unwrap() {}
                    }
                }
            });
        }

        Ok(())
    })
}

fn is_pop_item(is: &mut SharedRequest) -> Result<shared::RequestData, anyhow::Error> {
    unsafe {
        sem_wait(is.count).r("wait_count")?;
        sem_wait(is.lock).r("wait_lock")?;

        let item = &mut (*is.buffer)[(*is.read) & (REQ_BUFFER_SIZE - 1)];

        let data = item.assume_init();
        (*is.read) = (*is.read).wrapping_add(1);

        sem_post(is.lock).r("post_lock")?;
        (*is.space).post();

        anyhow::Ok(data)
    }
}

fn os_push_item(item: ResponseData, os: &mut SharedResponse) -> Result<bool, anyhow::Error> {
    unsafe {
        pthread_mutex_lock(os.tail_lock).r("wait_tail")?;

        if *os.tail_rx_cnt == 0 {
            pthread_mutex_unlock(os.tail_lock).r("post_tail")?;
            eprintln!("All clients left the channel, dropping msg: {item:?}");
            return Ok(true);
        }

        let pos = *os.tail_pos;
        let rem = *os.tail_rx_cnt;

        let id = (pos & (RES_BUFFER_SIZE - 1) as u64) as usize;

        let slot = (*os.buffer)[id].assume_init_mut();
        let slot_lock = &raw mut slot.lock;

        pthread_rwlock_wrlock(slot_lock).r("wait_slot")?;

        if slot.rem.load(Ordering::Relaxed) > 0 {
            pthread_rwlock_unlock(slot_lock).r("post_slot")?;
            pthread_mutex_unlock(os.tail_lock).r("post_tail")?;
            return Ok(false);
        }

        *os.tail_pos = (*os.tail_pos).wrapping_add(1);

        slot.pos = pos;
        slot.rem.store(rem, Ordering::Relaxed);

        slot.val.write(item);

        pthread_rwlock_unlock(slot_lock).r("post_slot")?;

        pthread_mutex_unlock(os.tail_lock).r("post_tail")?;

        anyhow::Ok(true)
    }
}
