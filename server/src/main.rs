use std::{mem::MaybeUninit, process::exit, thread};

use anyhow::bail;
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
    SharedRequest, SharedResponse, MAGIC_VALUE, REQ_BUFFER_SIZE, SHM_REQUEST, SHM_RESPONSE,
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
        sem_init(os.readers, 1, 1).r("init_global")?;
        sem_init(os.barrier1, 1, 0).r("init_barrier1")?;
        sem_init(os.barrier2, 1, 1).r("init_barrier1")?;
        sem_init(os.read_complete, 1, 0).r("init_read_complete")?;
        sem_init(os.write_complete, 1, 0).r("init_write_complete")?;
        sem_init(os.count_mutex, 1, 1).r("init_count_mutex")?;
    }

    unsafe {
        (*is.buffer) = [MaybeUninit::uninit(); 1024];
        (*is.read) = 0;
        (*is.write) = 0;

        (*os.num_readers) = 0;
        (*os.count) = 0;

        (*is.magic) = MAGIC_VALUE;
        (*os.magic) = MAGIC_VALUE;
    }

    println!("Initialized [{} -> {}]", SHM_REQUEST, SHM_RESPONSE);

    ctrlc::set_handler(move || {
        shm::unlink(SHM_REQUEST).unwrap();
        shm::unlink(SHM_RESPONSE).unwrap();
        exit(0);
    })?;

    let pop_item = |is: SharedRequest| unsafe {
        sem_wait(is.count).r("wait_count")?;
        sem_wait(is.lock).r("wait_lock")?;

        let item = &mut (*is.buffer)[(*is.read) & (REQ_BUFFER_SIZE - 1)];

        let data = item.assume_init();
        (*is.read) = (*is.read).wrapping_add(1);

        sem_post(is.lock).r("post_lock")?;
        sem_post(is.space).r("post_space")?;

        anyhow::Ok(data)
    };

    let (snd_out, output) = crossbeam_channel::unbounded();

    println!("Server is ready to accept connections");

    thread::scope(|s| {
        let output_thread = s.spawn(move || {
            let os = os;
            loop {
                let next_message: ResponseData = output.recv().unwrap();
                unsafe {
                    // Lock join / leave
                    sem_wait(os.readers).r("wait_readers")?;
                    // Get current number of readers
                    let nr = os.num_readers.read_volatile();
                    // Send data
                    os.data.write_volatile(next_message);

                    // Wake up all readers
                    for _ in 0..nr {
                        sem_post(os.write_complete).r("post_wc")?;
                    }
                    // If there is at least one reader, wait for the barrier finalization signal / thread
                    if nr > 0 {
                        sem_wait(os.read_complete).r("wait_rc")?;
                    }
                    // Unlock join / leave
                    if sem_post(os.readers) != 0 {
                        bail!("post_readers");
                    }
                }
            }
        });

        for i in 0..args.num_threads {
            let _worker = format!("{i}");
            s.spawn(|| {
                let is = is;
                loop {
                    if let Ok(request) = pop_item(is) {
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

                        snd_out.send(response).unwrap();
                    }
                }
            });
        }

        output_thread.join().unwrap()?;

        Ok(())
    })
}
