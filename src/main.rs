use clap::Parser;
use searchup::{format_report, run_analysis, AnalysisConfig, SearchMode};
use std::path::PathBuf;
use std::process;

#[derive(Parser, Debug)]
#[command(
    name = "searchup",
    author = "SearchUp",
    version,
    about = "High-performance parallel CSV analyzer",
    long_about = "A fast CLI tool that utilizes parallel worker threads to analyze CSV files, detect missing values, and inspect data completeness."
)]
struct Args {
    /// Path to the CSV file to analyze
    #[arg(short, long, value_name = "FILE")]
    load: PathBuf,

    /// Target to search (e.g. 'missing_values', 'missing values', or text query)
    #[arg(short, long, num_args(1..), value_name = "TARGET")]
    search: Vec<String>,

    /// Number of parallel worker threads (defaults to available CPU cores)
    #[arg(short, long, value_name = "NUM")]
    workers: Option<usize>,

    /// Batch size for worker chunks (default: 4096)
    #[arg(long, default_value_t = 4096)]
    batch_size: usize,
}

fn main() {
    let args = Args::parse();

    if let Some(w) = args.workers {
        if w == 0 {
            eprintln!("Error: --workers must be greater than 0");
            process::exit(1);
        }
    }

    if !args.load.exists() {
        eprintln!("Error: File '{}' does not exist.", args.load.display());
        process::exit(1);
    }

    if args.search.is_empty() {
        eprintln!("Error: Please provide a search target (e.g. --search missing_values)");
        process::exit(1);
    }

    let search_query = args.search.join(" ");
    let mode = SearchMode::parse(&search_query);

    let default_workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let workers = args.workers.unwrap_or(default_workers);

    let mut config = AnalysisConfig::new(args.load, mode, workers);
    config.batch_size = args.batch_size;

    match run_analysis(&config) {
        Ok(result) => {
            let report = format_report(&result);
            print!("{}", report);
        }
        Err(err) => {
            eprintln!("Error during analysis: {}", err);
            process::exit(1);
        }
    }
}
