use clap::Parser;

/// HashTable Client
#[derive(Debug, Parser)]
pub struct Args {
    /// Outer loop iterations (number of measurements)
    ///
    /// Set to 0 for infinite iterations
    #[arg(default_value_t = 1000)]
    pub outer_iterations: usize,

    /// Inner loop iterations (number of requests per pass)
    #[arg(default_value_t = 100)]
    pub inner_iterations: usize,

    /// Start Seed for the random keys
    #[arg(long)]
    pub seed: Option<u32>,
}
