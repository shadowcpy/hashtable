use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
        Arc,
    },
    thread::{self, JoinHandle},
};

use anyhow::bail;
use rand::Rng;

use shared::{
    shm::SharedMemory, HashtableMemory, RequestData, RequestPayload, ResponseData, ResponseFrame,
    DESCRIPTOR, REQ_BUFFER_SIZE, RES_BUFFER_SIZE,
};

pub struct HashtableClient {
    client_id: u32,
    mem: Arc<SharedMemory<HashtableMemory>>,
    shutdown: Arc<AtomicBool>,
    responses: Receiver<ResponseData>,
    response_thread: Option<JoinHandle<anyhow::Result<()>>>,
}

impl HashtableClient {
    pub unsafe fn init() -> anyhow::Result<Self> {
        let mem = Arc::new(SharedMemory::join(DESCRIPTOR)?);

        let mut rng = rand::thread_rng();
        let client_id: u32 = rng.gen();

        let (snd_responses, responses) = mpsc::channel();

        let shutdown = Arc::new(AtomicBool::new(false));
        let s = shutdown.clone();

        let mut read_next;
        {
            let mem: &HashtableMemory = mem.get();
            let mut tail = mem.response_frame.tail.lock();
            tail.rx_cnt = tail.rx_cnt.checked_add(1).unwrap();
            read_next = tail.pos;
        }

        let imem = mem.clone();
        let response_thread = thread::spawn(move || {
            let is = &imem.get().response_frame;

            while !s.load(Ordering::Relaxed) {
                let msg = Self::inner_try_recv(&mut read_next, is);
                if let Some(msg) = msg {
                    if msg.client_id != client_id {
                        continue;
                    }
                    if snd_responses.send(msg).is_err() {
                        break;
                    }
                }
            }

            // Safety: Shuts down the client, leaving the response stream
            // Must not be called twice, and must be called before exiting (drop will automatically call it)

            let mut tail = is.tail.lock();
            tail.rx_cnt -= 1;
            let until = tail.pos;

            while read_next < until {
                match Self::inner_try_recv(&mut read_next, is) {
                    Some(_) => {}
                    None => panic!("empty channel?"),
                }
            }
            eprintln!("Left session");

            anyhow::Ok(())
        });

        Ok(Self {
            client_id,
            mem,
            responses,
            response_thread: Some(response_thread),
            shutdown,
        })
    }

    pub fn send(&mut self, request: RequestPayload, id: u32) {
        let os = &self.mem.get().request_frame;
        os.space.wait();

        let mut queue = os.queue.lock();

        let qid = queue.write & (REQ_BUFFER_SIZE - 1);
        queue.buffer[qid].write(RequestData {
            client_id: self.client_id,
            request_id: id,
            payload: request,
        });

        queue.write = queue.write.wrapping_add(1);
        os.count.post();
    }

    pub fn try_recv(&mut self) -> anyhow::Result<Option<ResponseData>> {
        let val = self.responses.try_recv();
        match val {
            Ok(t) => Ok(Some(t)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(_) => bail!("recv error"),
        }
    }

    fn inner_try_recv(read_next: &mut u64, is: &ResponseFrame) -> Option<ResponseData> {
        let id = (*read_next & (RES_BUFFER_SIZE - 1) as u64) as usize;
        let lock = unsafe { is.buffer[id].assume_init_ref() };
        let slot = lock.read();

        if slot.pos != *read_next {
            drop(slot);
            let tail = is.tail.lock();
            drop(tail);
            return None;
        }

        *read_next = read_next.wrapping_add(1);
        let value = unsafe { slot.val.assume_init_read() };
        let orig_rem = slot.rem.fetch_sub(1, Ordering::Relaxed);
        if orig_rem == 1 {
            // Last receiver, drop
            unsafe { lock.bypass().val.assume_init_drop() };
        }

        return Some(value);
    }

    pub fn shutdown(&mut self) -> anyhow::Result<()> {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(t) = self.response_thread.take() {
            t.join().unwrap()?;
        }
        Ok(())
    }
}

impl Drop for HashtableClient {
    fn drop(&mut self) {
        self.shutdown().unwrap()
    }
}
