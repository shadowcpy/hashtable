use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
        Arc,
    },
    thread::{self, JoinHandle},
};

use anyhow::{bail, Context};
use libc::{
    pthread_mutex_lock, pthread_mutex_unlock, pthread_rwlock_rdlock, pthread_rwlock_t,
    pthread_rwlock_unlock, sem_post, sem_wait,
};
use rand::Rng;
use rustix::{
    fs::Mode,
    shm::{self, OFlags},
};

use shared::{
    CheckOk, RequestData, RequestPayload, ResponseData, SharedRequest, SharedResponse, MAGIC_VALUE,
    REQ_BUFFER_SIZE, RES_BUFFER_SIZE, SHM_REQUEST, SHM_RESPONSE,
};

pub struct HashtableClient {
    client_id: u32,
    os: SharedRequest,
    shutdown: Arc<AtomicBool>,
    responses: Receiver<ResponseData>,
    response_thread: Option<JoinHandle<anyhow::Result<()>>>,
}

impl HashtableClient {
    pub unsafe fn init() -> anyhow::Result<Self> {
        let req_fd = shm::open(SHM_REQUEST, OFlags::RDWR, Mode::RUSR | Mode::WUSR)
            .context("Opening shared memory failed")?;

        let res_fd = shm::open(SHM_RESPONSE, OFlags::RDWR, Mode::RUSR | Mode::WUSR)
            .context("Opening shared memory failed")?;

        let os = SharedRequest::from_fd(req_fd)?;
        let is = SharedResponse::from_fd(res_fd)?;

        if unsafe { *is.magic } != MAGIC_VALUE || unsafe { *os.magic } != MAGIC_VALUE {
            bail!("Server not ready yet");
        }

        let mut rng = rand::thread_rng();
        let client_id: u32 = rng.gen();

        let (snd_responses, responses) = mpsc::channel();

        let shutdown = Arc::new(AtomicBool::new(false));
        let s = shutdown.clone();

        let mut read_next;
        unsafe {
            pthread_mutex_lock(is.tail_lock).r("wait_tail")?;
            *is.tail_rx_cnt = (*is.tail_rx_cnt).checked_add(1).unwrap();
            read_next = *is.tail_pos;

            pthread_mutex_unlock(is.tail_lock).r("post_tail")?;
        }

        let response_thread = thread::spawn(move || {
            let mut is = is;

            while !s.load(Ordering::Relaxed) {
                let msg = Self::inner_try_recv(&mut read_next, &mut is)?;
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

            pthread_mutex_lock(is.tail_lock).r("wait_tail")?;

            *is.tail_rx_cnt -= 1;
            let until = *is.tail_pos;

            pthread_mutex_unlock(is.tail_lock).r("post_tail")?;

            while read_next < until {
                match Self::inner_try_recv(&mut read_next, &mut is) {
                    Ok(Some(_)) => {}
                    Ok(None) => panic!("empty channel?"),
                    Err(e) => {
                        eprintln!("encountered leave error {e}")
                    }
                }
            }
            eprintln!("Left session");

            anyhow::Ok(())
        });

        Ok(Self {
            client_id,
            os,
            responses,
            response_thread: Some(response_thread),
            shutdown,
        })
    }

    pub unsafe fn send(&mut self, request: RequestPayload, id: u32) -> anyhow::Result<()> {
        sem_wait(self.os.space).r("wait_space")?;
        sem_wait(self.os.lock).r("wait_lock")?;

        let item = &mut (*self.os.buffer)[(*self.os.write) & (REQ_BUFFER_SIZE - 1)];

        item.write(RequestData {
            client_id: self.client_id,
            request_id: id,
            payload: request,
        });

        *self.os.write = (*self.os.write).wrapping_add(1);

        sem_post(self.os.lock).r("post_lock")?;
        sem_post(self.os.count).r("post_count")?;
        anyhow::Ok(())
    }

    pub unsafe fn unlock_mis(sl: *mut pthread_rwlock_t) -> anyhow::Result<()> {
        pthread_rwlock_unlock(sl).r("post_slot")?;
        Ok(())
    }

    pub fn try_recv(&mut self) -> anyhow::Result<Option<ResponseData>> {
        let val = self.responses.try_recv();
        match val {
            Ok(t) => Ok(Some(t)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(_) => bail!("recv error"),
        }
    }

    unsafe fn inner_try_recv(
        read_next: &mut u64,
        is: &mut SharedResponse,
    ) -> anyhow::Result<Option<ResponseData>> {
        let id = (*read_next & (RES_BUFFER_SIZE - 1) as u64) as usize;
        let slot = (*is.buffer)[id].assume_init_mut();

        let slot_lock = &raw mut slot.lock;

        pthread_rwlock_rdlock(slot_lock).r("wait_slot")?;

        if slot.pos != *read_next {
            pthread_rwlock_unlock(slot_lock).r("post_slot")?;
            pthread_mutex_lock(is.tail_lock).r("wait_tail")?;
            pthread_mutex_unlock(is.tail_lock).r("post_tail")?;
            return Ok(None);
        }

        *read_next = read_next.wrapping_add(1);
        let value = slot.val.assume_init_read();
        let orig_rem = slot.rem.fetch_sub(1, Ordering::Relaxed);
        if orig_rem == 1 {
            // Last receiver, drop
            slot.val.assume_init_drop();
        }
        pthread_rwlock_unlock(slot_lock).r("post_slot")?;
        return Ok(Some(value));
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
