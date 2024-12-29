use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use anyhow::bail;
use arrayvec::ArrayString;
use clap::Parser;
use client::HashtableClient;
use rand::Rng;

pub mod cli;
pub mod client;

use cli::Args;
use shared::{RequestPayload, ResponsePayload};

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

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

    let recv = |client: &mut HashtableClient| {
        let response = loop {
            match client.try_recv()? {
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

    let send = |client: &mut HashtableClient, request, id| client.send(request, id);

    // Outer Iterations: Number of runs: Insert Read Delete
    let mut outer_iter = 0;
    // Inner Iterations: Number of values to be inserted
    let inner_iter = args.inner_iterations;

    // Create Hashmap for verifying all requests later
    let mut rmap = HashMap::new();
    // Buffer to hold the values we are going to store
    let mut buffer = vec![ArrayString::<64>::new(); inner_iter];

    let seed: u32 = args.seed.unwrap_or_else(|| rng.gen());
    println!("Seed: {seed}");
    loop {
        if (args.outer_iterations > 0 && outer_iter == args.outer_iterations)
            || exit_signal.load(Ordering::Relaxed)
        {
            break;
        }

        for i in 0..inner_iter {
            let suffix: u32 = rng.gen();
            let name = format!("ht{seed}{suffix}");
            buffer[i] = ArrayString::new();
            buffer[i].push_str(&name);
        }

        let mut copy = buffer.clone();
        copy.sort();
        copy.dedup();

        let mut duplicates = (buffer.len() - copy.len()) as isize;

        // Insert random numbers
        for i in 0..inner_iter {
            let val = buffer[i];
            send(client, RequestPayload::Insert(val, i as u32), i as u32);
        }

        // Split send and receive to allow for server concurrency

        for i in 0..inner_iter {
            let response = recv(client)?;
            let ResponsePayload::Inserted = response.payload else {
                bail!("Invalid response for insert request {i}");
            };
        }

        // Verify that all values are correct
        // Send read request to HashMap
        for i in 0..inner_iter {
            send(client, RequestPayload::ReadBucket(buffer[i]), i as u32);
        }

        // Get read responses
        for i in 0..inner_iter {
            let response = recv(client)?;
            let ResponsePayload::BucketContent { len, data } = response.payload else {
                bail!("Invalid response for read request {i}");
            };

            rmap.insert(response.request_id, data[..len].to_vec());
        }

        // Compare for equality, bucket must contain value
        for i in 0..inner_iter {
            let expected = buffer[i];
            let Some(v) = rmap.get(&(i as u32)) else {
                panic!("Missing response for read request {i}");
            };
            let value = v.iter().find(|(k, v)| *k == expected && *v == i as u32);
            let Some(_) = value else {
                bail!("missing value in bucket {expected}");
            };
        }

        // Delete values again
        for i in 0..inner_iter {
            let val = buffer[i];

            send(client, RequestPayload::Delete(val), i as u32);
        }

        for i in 0..inner_iter {
            let response = recv(client)?;
            match response.payload {
                ResponsePayload::Deleted => continue,
                ResponsePayload::NotFound => {
                    duplicates -= 1;
                    if duplicates < 0 {
                        bail!("element was wrongly deleted: {i}");
                    }
                }
                _ => bail!("invalid deletion response"),
            }
        }

        rmap.clear();
        outer_iter += 1;
    }
    Ok(())
}
