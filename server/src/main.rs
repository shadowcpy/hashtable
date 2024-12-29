use std::{process::exit, sync::atomic::Ordering, thread};

use clap::Parser;

use rustix::shm;

pub mod cli;
pub mod hash_table;

use cli::Args;
use hash_table::HashTable;
use shared::{
    shm::SharedMemory, HashtableMemory, KeyType, RequestFrame, RequestPayload, ResponseData,
    ResponseFrame, ResponsePayload, DESCRIPTOR, REQ_BUFFER_SIZE, RES_BUFFER_SIZE,
};

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let mem = SharedMemory::create(DESCRIPTOR, |mem| unsafe {
        HashtableMemory::init_in_shm(mem.as_mut_ptr(), args.num_threads);
    })?;

    let hm: HashTable<KeyType, u32> = HashTable::new(args.size);

    println!("Initialized {}", DESCRIPTOR);

    ctrlc::set_handler(move || {
        shm::unlink(DESCRIPTOR).unwrap();
        exit(0);
    })?;

    println!("Server is ready to accept connections");

    thread::scope(|s| {
        for i in 0..args.num_threads {
            let _worker = format!("{i}");
            s.spawn(|| {
                let mem = mem.get();
                loop {
                    let request = is_pop_item(&mem.request_frame);
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

                    os_push_item(response, &mem.response_frame);
                }
            });
        }

        Ok(())
    })
}

fn is_pop_item(is: &RequestFrame) -> shared::RequestData {
    is.count.wait();

    let mut queue = is.queue.lock();

    let id = queue.read & (REQ_BUFFER_SIZE - 1);
    let item = &mut queue.buffer[id];

    let data = unsafe { item.assume_init() };

    queue.read = queue.read.wrapping_add(1);

    drop(queue);

    is.space.post();
    data
}

fn os_push_item(item: ResponseData, os: &ResponseFrame) {
    os.space.wait();

    let mut tail = os.tail.lock();

    if tail.rx_cnt == 0 {
        eprintln!("All clients left the channel, dropping msg: {item:?}");
        return;
    }

    let pos = tail.pos;
    let rem = tail.rx_cnt;

    let id = (pos & (RES_BUFFER_SIZE - 1) as u64) as usize;

    let lock = &os.buffer[id];
    let mut slot = lock.write();

    tail.pos = tail.pos.wrapping_add(1);

    slot.pos = pos;
    slot.rem.store(rem, Ordering::Relaxed);

    slot.val.write(item);
}
