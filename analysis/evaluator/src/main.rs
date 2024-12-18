#[derive(serde::Deserialize)]
struct Record {
    mean: f64,
}

fn main() {
    let alone_st = load_csv("AloneSingleThread");
    let alone_mt = load_csv("AloneMultiThread");
    let alone_mat = load_csv("SCManyThreads");
    let two_mt = load_csv("TwoMultiThread");
    let two_st = load_csv("TwoSingleThread");
    let many_st = load_csv("ManyClientsST");
    let many_mt = load_csv("ManyClientsMT");

    let text = |f: f64| {
        if f > 1. {
            "is faster than"
        } else if f < 1. {
            "is slower than"
        } else {
            "is equal to"
        }
    };

    println!("--- Summary ---");
    println!();

    let alone_mt_st = 1.0 / (alone_mt.mean / alone_st.mean);
    println!(
        "alone at 4 threads {} alone at 1 thread: {:.02}x ",
        text(alone_mt_st),
        alone_mt_st,
    );

    let alone_mat_st = 1.0 / (alone_mat.mean / alone_st.mean);
    println!(
        "alone at 32 threads {} alone at 1 thread: {:.02}x ",
        text(alone_mat_st),
        alone_mat_st,
    );

    println!();

    let alone_two_st = 1.0 / (alone_st.mean / two_st.mean);
    println!(
        "alone at 1 thread {} one visitor at 1 thread: {:.02}x",
        text(alone_two_st),
        alone_two_st,
    );

    let alone_many_st = 1.0 / (alone_st.mean / many_st.mean);
    println!(
        "alone at 1 thread {} 16 visitors at 1 thread: {:.02}x",
        text(alone_many_st),
        alone_many_st,
    );

    println!();

    let alone_two_mt = 1.0 / (alone_mt.mean / two_mt.mean);
    println!(
        "alone at 4 threads {} one visitor at 4 threads: {:.02}x",
        text(alone_two_mt),
        alone_two_mt,
    );

    let alone_many_mt = 1.0 / (alone_mt.mean / many_mt.mean);
    println!(
        "alone at 4 threads {} 16 visitors at 4 threads: {:.02}x",
        text(alone_many_mt),
        alone_many_mt,
    );
}

fn load_csv(name: &str) -> Record {
    let mut reader = csv::Reader::from_path(format!("analysis/benchmarks/{name}.csv")).unwrap();
    let list: Result<Vec<Record>, _> = reader.deserialize().collect();
    list.unwrap().remove(0)
}
