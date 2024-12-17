use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::SystemTime,
};

use clap::Parser;
use client::HashtableClient;
use rand::Rng;

pub mod cli;
pub mod client;

use cli::Args;
use shared::{RequestPayload, ResponsePayload};
use tracing::Level;

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .init();

    let exit_signal = Arc::new(AtomicBool::new(false));

    let e = exit_signal.clone();
    ctrlc::set_handler(move || {
        if e.swap(true, Ordering::Relaxed) {
            eprintln!("Killing");
            std::process::exit(1);
        } else {
            eprintln!("CTRL-C received, terminating (press again to kill)");
        }
    })?;

    unsafe {
        let mut client = HashtableClient::init()?;
        benchmark(&args, &mut client, exit_signal)?;
    }

    Ok(())
}

fn benchmark(
    args: &Args,
    client: &mut HashtableClient,
    exit_signal: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let mut rng = rand::thread_rng();
    let mut rng2 = rand::thread_rng();

    let mut recv = |client: &mut HashtableClient| unsafe {
        let response = loop {
            match client.try_recv(rng2.gen())? {
                Some(response) => break response,
                None => {
                    if exit_signal.load(Ordering::Relaxed) {
                        client.shutdown()?;
                        std::process::exit(0);
                    }
                }
            };
        };
        anyhow::Ok(response)
    };

    let send = |client: &mut HashtableClient, request, id| unsafe { client.send(request, id) };

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

        //let start = SystemTime::now();
        for i in 0..inner_iter {
            let val = buffer[i];
            send(client, RequestPayload::Insert(val, val), i as u32)?;
        }

        for i in 0..inner_iter {
            let response = recv(client)?;
            let ResponsePayload::Inserted(a) = response.payload else {
                panic!("Invalid response for request {i}");
            };
            rmap.insert(response.request_id, a);
        }

        //let pass = SystemTime::now().duration_since(start).unwrap();
        // println!("{}", pass.as_nanos());

        for i in 0..inner_iter {
            let Some(v) = rmap.get(&(i as u32)) else {
                panic!("Missing response for request {i}");
            };
            assert_eq!(*v, buffer[i]);
        }

        for i in 0..inner_iter {
            let val = buffer[i];

            send(client, RequestPayload::Delete(val), i as u32)?;
        }

        for i in 0..inner_iter {
            let response = recv(client)?;
            let ResponsePayload::Deleted = response.payload else {
                panic!("Invalid response for request {i}");
            };
        }

        rmap.clear();
        outer_iter += 1;
    }
    Ok(())
}
