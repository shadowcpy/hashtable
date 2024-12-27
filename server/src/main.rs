use std::{mem::MaybeUninit, process::exit, thread};

use clap::Parser;
use libc::{sem_init, sem_post, sem_wait};
use rustix::{
    fs::{ftruncate, Mode},
    shm::{self, OFlags},
};

pub mod cli;
pub mod hash_table;

use cli::Args;
use hash_table::HashTable;
use shared::{
    CheckOk, KeyType, RequestFrame, RequestPayload, ResponseData, ResponseFrame, ResponsePayload,
    SharedRequest, SharedResponse, MAGIC_VALUE, REQ_BUFFER_SIZE, RES_BUFFER_SIZE, SHM_REQUEST,
    SHM_RESPONSE,
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
        sem_init(is.space, 1, REQ_BUFFER_SIZE as u32).r("init_space")?;
        sem_init(is.lock, 1, 1).r("init_lock")?;
    }

    // Responses (output)
    unsafe {
        sem_init(os.tail_lock, 1, 1).r("init_taillock")?;
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
            (*slot).rem = 0;
            (*slot).val = MaybeUninit::uninit();

            sem_init(&raw mut (*slot).lock, 1, 1).r("slot_init")?;
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
        sem_post(is.space).r("post_space")?;

        anyhow::Ok(data)
    }
}

fn os_push_item(item: ResponseData, os: &mut SharedResponse) -> Result<bool, anyhow::Error> {
    unsafe {
        sem_wait(os.tail_lock).r("wait_tail")?;

        if *os.tail_rx_cnt == 0 {
            sem_post(os.tail_lock).r("post_tail")?;
            eprintln!("All clients left the channel, dropping msg: {item:?}");
            return Ok(true);
        }

        let pos = *os.tail_pos;
        let rem = *os.tail_rx_cnt;

        let id = (pos & (RES_BUFFER_SIZE - 1) as u64) as usize;

        let slot = (*os.buffer)[id].assume_init_mut();
        let slot_lock = &raw mut slot.lock;

        sem_wait(slot_lock).r("wait_slot")?;

        if slot.rem > 0 {
            sem_post(slot_lock).r("post_slot")?;
            sem_post(os.tail_lock).r("post_tail")?;
            return Ok(false);
        }

        *os.tail_pos = (*os.tail_pos).wrapping_add(1);

        slot.pos = pos;
        slot.rem = rem;

        slot.val.write(item);

        sem_post(slot_lock).r("post_slot")?;

        // Notify here?
        sem_post(os.tail_lock).r("post_tail")?;

        anyhow::Ok(true)
    }
}
