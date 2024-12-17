use std::time::Duration;

use anyhow::{bail, Context};
use libc::{sem_post, sem_wait, ETIMEDOUT};
use rand::Rng;
use rustix::{
    fs::Mode,
    shm::{self, OFlags},
};

use shared::{
    sema_trywait, sema_wait_timeout, CheckOk, RequestData, RequestPayload, ResponseData,
    SharedRequest, SharedResponse, MAGIC_VALUE, SHM_REQUEST, SHM_RESPONSE,
};
use tracing::{instrument, trace};

pub struct HashtableClient {
    client_id: u32,
    os: SharedRequest,
    is: SharedResponse,
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

        unsafe {
            sem_wait(is.readers).r("wait_readers")?;
            eprintln!("Joining session with {} other clients", *is.num_readers);
            *is.num_readers += 1;
            sem_post(is.readers).r("post_readers")?;
        }

        Ok(Self { client_id, os, is })
    }

    pub unsafe fn send(&mut self, request: RequestPayload, id: u32) -> anyhow::Result<()> {
        sem_wait(self.os.busy).r("wait_busy")?;

        *self.os.data = RequestData {
            client_id: self.client_id,
            request_id: id,
            payload: request,
        };

        sem_post(self.os.waker).r("post_waker")?;
        anyhow::Ok(())
    }

    #[instrument(skip(self))]
    pub unsafe fn try_recv(&mut self, id: u32) -> anyhow::Result<Option<ResponseData>> {
        let is = &self.is;
        loop {
            match sema_wait_timeout(is.write_complete, Duration::from_millis(20)) {
                0 => break,
                ETIMEDOUT => {
                    return Ok(None);
                }
                e => bail!("wait_write_complete: {e}"),
            }
        }

        trace!("L WriteComplete");
        sem_wait(is.count_mutex).r("wait_count_mutex")?;
        trace!("L Count");
        *is.count += 1;
        let n = *is.num_readers;
        if *is.count == n {
            sem_wait(is.barrier2).r("wait_barrier2")?;
            trace!("L Barrier2 *");
            sem_post(is.barrier1).r("post_barrier1")?;
            trace!("P Barrier1 *");
        }

        sem_post(is.count_mutex).r("post_count_mutex")?;
        trace!("P Count");

        sem_wait(is.barrier1).r("wait_barrier1")?;
        trace!("L Barrier1");

        sem_post(is.barrier1).r("post_barrier1")?;
        trace!("P Barrier1");

        let data = is.data.read_volatile();
        let data = if data.client_id == self.client_id {
            Some(data)
        } else {
            None
        };

        trace!("R Data");

        sem_wait(is.count_mutex).r("wait_count_mutex")?;
        trace!("L Count");
        *is.count -= 1;
        if *is.count == 0 {
            sem_wait(is.barrier1).r("wait_barrier1")?;
            trace!("L Barrier1 *");
            sem_post(is.barrier2).r("post_barrier2")?;
            trace!("P Barrier2 *");
            sem_post(is.read_complete).r("post_read_complete")?;
            trace!("P ReadComplete *");
        }
        sem_post(is.count_mutex).r("post_count_mutex")?;
        trace!("P Count");

        sem_wait(is.barrier2).r("wait_barrier2")?;
        trace!("L Barrier2");
        sem_post(is.barrier2).r("post_barrier2")?;
        trace!("P Barrier2");
        return Ok(data);
    }

    /// Safety: Shuts down the client, leaving the response stream
    /// Must not be called twice, and must be called before exiting (drop will automatically call it)
    pub unsafe fn shutdown(&mut self) -> anyhow::Result<()> {
        loop {
            let exit_attempt = unsafe { self.try_exit()? };
            if exit_attempt {
                return Ok(());
            } else {
                let _ = self.try_recv(1);
            }
        }
    }

    /// Try to exit the current session
    /// Returns true if successful
    unsafe fn try_exit(&self) -> anyhow::Result<bool> {
        let is = &self.is;
        // If we want to leave, try to lock num_readers,
        // before the server can start the next round
        if sema_trywait(is.readers) == 0 {
            *is.num_readers -= 1;
            sem_post(is.readers).r("post_readers")?;
            eprintln!("Left session");
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

impl Drop for HashtableClient {
    fn drop(&mut self) {
        unsafe { self.shutdown().unwrap() }
    }
}
