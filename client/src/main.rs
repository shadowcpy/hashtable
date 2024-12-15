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
use clap::Parser;
use libc::{sem_post, sem_wait, ETIMEDOUT};
use rand::Rng;
use rustix::{
    fs::Mode,
    shm::{self, OFlags},
};

pub mod cli;

use cli::Args;
use shared::{
    sema_trywait, sema_wait_timeout, CheckOk, RequestData, RequestPayload, ResponsePayload,
    SharedRequest, SharedResponse, MAGIC_VALUE, SHM_REQUEST, SHM_RESPONSE,
};

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

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
                // If we want to leave, try to lock num_readers,
                // before the server can start the next round
                if e.load(Ordering::Relaxed) {
                    if sema_trywait(is.readers) == 0 {
                        return exit_input(is);
                    }
                }
                loop {
                    match sema_wait_timeout(is.write_complete, Duration::from_millis(20)) {
                        0 => break,
                        ETIMEDOUT => {
                            if e.load(Ordering::Relaxed) {
                                if sema_trywait(is.readers) == 0 {
                                    return exit_input(is);
                                }
                            }
                        }
                        e => bail!("wait_write_complete: {e}"),
                    }
                }

                sem_wait(is.count_mutex).r("wait_count_mutex")?;
                *is.count += 1;
                let n = *is.num_readers;
                if *is.count == n {
                    sem_wait(is.barrier2).r("wait_barrier2")?;
                    sem_post(is.barrier1).r("post_barrier1")?;
                }
                sem_post(is.count_mutex).r("post_count_mutex")?;

                sem_wait(is.barrier1).r("wait_barrier1")?;
                sem_post(is.barrier1).r("post_barrier1")?;

                let data = is.data.read_volatile();
                if data.client_id == client_id {
                    snd_in.send(data).unwrap();
                }

                sem_wait(is.count_mutex).r("wait_count_mutex")?;
                *is.count -= 1;
                if *is.count == 0 {
                    sem_wait(is.barrier1).r("wait_barrier1")?;
                    sem_post(is.barrier2).r("post_barrier2")?;
                    sem_post(is.read_complete).r("post_read_complete")?;
                }
                sem_post(is.count_mutex).r("post_count_mutex")?;

                sem_wait(is.barrier2).r("wait_barrier2")?;
                sem_post(is.barrier2).r("post_barrier2")?;
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
                        return anyhow::Ok(());
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };

            if e.load(Ordering::Relaxed) {
                return Ok(());
            }

            unsafe {
                sem_wait(os.busy).r("wait_busy")?;

                *os.data = request;
                sem_post(os.waker).r("post_waker")?;
            }
        }
        Ok(())
    });

    let mut outer_iter = 0;
    let inner_iter = args.inner_iterations;

    let mut rmap = HashMap::new();
    let mut buffer = vec![0u32; inner_iter];
    loop {
        if (args.outer_iterations > 0 && outer_iter == args.outer_iterations)
            || exit_signal.load(Ordering::Relaxed)
        {
            break;
        }

        for i in 0..inner_iter {
            buffer[i] = rng.gen();
        }

        let start = SystemTime::now();
        for i in 0..inner_iter {
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

        for i in 0..inner_iter {
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

        for i in 0..inner_iter {
            let Some(v) = rmap.get(&(i as u32)) else {
                panic!("Missing response for request {i}");
            };
            assert_eq!(*v, buffer[i]);
        }

        for i in 0..inner_iter {
            let val = buffer[i];
            let request = RequestData {
                client_id,
                request_id: i as u32,
                payload: RequestPayload::Delete(val),
            };
            if snd_out.send(request).is_err() {
                eprintln!("Terminating main loop");
                break;
            }
        }

        for i in 0..inner_iter {
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
            let ResponsePayload::Deleted = response.payload else {
                panic!("Invalid response for request {i}");
            };
        }

        rmap.clear();
        outer_iter += 1;
    }

    exit_signal.store(true, Ordering::Relaxed);

    input_thr.join().unwrap()?;
    output_thr.join().unwrap()?;
    Ok(())
}

pub unsafe fn exit_input(is: SharedResponse) -> anyhow::Result<()> {
    *is.num_readers -= 1;
    sem_post(is.readers).r("post_readers")?;
    eprintln!("Left session");
    Ok(())
}
