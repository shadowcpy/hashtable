use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
        Arc,
    },
    thread::{self},
    time::{Duration, SystemTime},
};

use anyhow::{bail, Context};
use libc::{sem_post, sem_wait, ETIMEDOUT};
use rand::Rng;
use rustix::{
    fs::Mode,
    shm::{self, OFlags},
};

use shared::{
    sem_wait_timeout, sema_getvalue, CheckOk, RequestData, RequestPayload, ResponsePayload,
    SharedRequest, SharedResponse, MAGIC_VALUE, SHM_REQUEST, SHM_RESPONSE,
};

fn main() -> anyhow::Result<()> {
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

    let exit_signal = Arc::new(AtomicBool::new(false));

    let e = exit_signal.clone();
    ctrlc::set_handler(move || {
        eprintln!("CTRL-C received, terminating");
        e.store(true, Ordering::Relaxed);
    })?;

    let (snd_in, input) = mpsc::channel();
    let (snd_out, output) = mpsc::channel();

    let e = exit_signal.clone();
    let input_thr = thread::spawn(move || {
        let is = is;
        loop {
            unsafe {
                if e.load(Ordering::Relaxed) {
                    return leave_response(is);
                }
                loop {
                    match sem_wait_timeout(is.start, Duration::from_millis(20)) {
                        0 => break,
                        ETIMEDOUT => {
                            if e.load(Ordering::Relaxed) {
                                return leave_response(is);
                            }
                        }
                        e => bail!("wait_start: {e}"),
                    }
                }

                debug_assert_eq!(sema_getvalue(is.end_ack).unwrap(), 0);

                let data = is.data.read_volatile();
                if data.client_id == client_id {
                    snd_in.send(data).unwrap();
                }

                sem_post(is.end).r("post_end")?;
                sem_wait(is.end_ack).r("wait_endack")?;
            }
        }
    });
    let e = exit_signal.clone();
    let output_thr = thread::spawn(move || {
        let os = os;
        loop {
            let request = output.recv_timeout(Duration::from_millis(20));
            let request = match request {
                Ok(request) => request,
                Err(RecvTimeoutError::Timeout) => {
                    if e.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
            //println!("SND: {request:?}");
            if e.load(Ordering::Relaxed) {
                return Ok(());
            }
            loop {
                match unsafe { sem_wait_timeout(os.busy, Duration::from_millis(20)) } {
                    0 => break,
                    ETIMEDOUT => {
                        if e.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                    }
                    e => bail!("wait_busy: {e}"),
                }
            }
            unsafe {
                debug_assert_eq!(sema_getvalue(os.busy).unwrap(), 0);

                *os.data = request;
                sem_post(os.waker).r("post_waker")?;
            }
        }
        Ok(())
    });

    let mut rmap = HashMap::new();
    let mut buffer = [0u32; 100];
    for _ in 0..1000 {
        for i in 0..100 {
            buffer[i] = rng.gen();
        }
        let start = SystemTime::now();
        for i in 0..100 {
            let val = buffer[i];
            let request = RequestData {
                client_id,
                request_id: i as u32,
                payload: RequestPayload::Insert(val, val),
            };
            if snd_out.send(request).is_err() {
                eprintln!("Terminating main loop");
                break;
            }
        }

        for i in 0..100 {
            let response = match input.recv_timeout(Duration::from_secs(1)) {
                Ok(response) => response,
                Err(RecvTimeoutError::Timeout) => {
                    panic!("Timed out waiting for response");
                }
                Err(RecvTimeoutError::Disconnected) => {
                    eprintln!("Terminating main loop");
                    break;
                }
            };
            let ResponsePayload::Inserted(a) = response.payload else {
                panic!("Invalid response for request {i}");
            };
            rmap.insert(response.request_id, a);
        }

        let pass = SystemTime::now().duration_since(start).unwrap();
        println!("{}", pass.as_nanos());

        for i in 0..100 {
            let Some(v) = rmap.get(&(i as u32)) else {
                panic!("Missing response for request {i}");
            };
            assert_eq!(*v, buffer[i]);
        }
    }

    exit_signal.store(true, Ordering::Relaxed);

    input_thr.join().unwrap()?;
    output_thr.join().unwrap()?;
    Ok(())
}

pub unsafe fn leave_response(is: SharedResponse) -> anyhow::Result<()> {
    sem_wait(is.readers).r("wait_readers")?;
    *is.num_readers -= 1;
    sem_post(is.readers).r("post_readers")?;
    eprintln!("Left session");
    Ok(())
}
