use clap::Parser;

/// HashTable Server
#[derive(Debug, Clone, Parser)]
pub struct Args {
    /// Size of hash table
    #[arg(short)]
    pub size: usize,
    /// Number of parallel processing threads
    #[arg(short, default_value_t = 4)]
    pub num_threads: usize,
}
