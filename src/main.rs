use clap::Parser;
use searchup::{
    format_fill_report, format_report, parse_coord, run_analysis, AnalysisConfig, FillConfig,
    FillStrategy, FillTarget, SearchMode,
};
use std::path::PathBuf;
use std::process;

#[derive(Parser, Debug)]
#[command(
    name = "searchup",
    author = "SearchUp",
    version,
    about = "High-performance parallel CSV analyzer and imputer",
    long_about = "A fast CLI tool that utilizes parallel worker threads to analyze CSV files, detect missing values, and fill/impute data completeness for AI agents."
)]
struct Args {
    /// Path to the CSV file to analyze or modify
    #[arg(short, long, value_name = "FILE")]
    load: PathBuf,

    /// Target to search (e.g. 'missing_values', 'missing values', or text query)
    #[arg(short, long, num_args(1..), value_name = "TARGET")]
    search: Option<Vec<String>>,

    /// Number of parallel worker threads (defaults to available CPU cores)
    #[arg(short, long, value_name = "NUM")]
    workers: Option<usize>,

    /// Batch size for worker chunks (default: 4096)
    #[arg(long, default_value_t = 4096)]
    batch_size: usize,

    /// Dump structured JSON output for AI agents and automation
    #[arg(long)]
    json: bool,

    /// Maximum occurrences to output in detail (default: 15; use 0 for unlimited)
    #[arg(long, default_value_t = 15)]
    limit: usize,

    /// Output ALL occurrences without truncating
    #[arg(short, long)]
    all: bool,

    // === Fill / Imputation Arguments ===

    /// Enable missing value fill / imputation mode
    #[arg(long)]
    fill: bool,

    /// Coordinate to fill, e.g. '5,3', '5,age', or '(5,3)'
    #[arg(long, value_name = "ROW,COL")]
    coord: Option<String>,

    /// Target row number (1-based line number, e.g. 5)
    #[arg(long, value_name = "ROW")]
    row: Option<usize>,

    /// Target column name or 1-based index (e.g. 'age' or 3)
    #[arg(long, value_name = "COL")]
    col: Option<String>,

    /// Imputed value or strategy ('mean', 'median', 'mode', or literal value)
    #[arg(long, value_name = "VALUE")]
    val: Option<String>,

    /// Destination file to write modified CSV
    #[arg(short, long, value_name = "FILE")]
    out: Option<PathBuf>,

    /// Overwrite the loaded CSV in-place safely via atomic temporary file
    #[arg(long)]
    in_place: bool,
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

    if args.fill {
        // === Fill / Imputation Mode ===
        let val_input = match &args.val {
            Some(v) => v,
            None => {
                if args.json {
                    println!(
                        r#"{{"status":"error","message":"--fill requires --val <value|mean|median|mode>"}}"#
                    );
                } else {
                    eprintln!("Error: --fill requires --val <value|mean|median|mode>");
                }
                process::exit(1);
            }
        };

        let strategy = FillStrategy::parse(val_input);

        // Resolve target row & col
        let (target_row, target_col) = if let Some(coord_str) = &args.coord {
            match parse_coord(coord_str) {
                Ok((r, c)) => (Some(r), c),
                Err(err) => {
                    if args.json {
                        println!(r#"{{"status":"error","message":"{}"}}"#, err);
                    } else {
                        eprintln!("Error: {}", err);
                    }
                    process::exit(1);
                }
            }
        } else if let Some(col_name) = &args.col {
            (args.row, col_name.clone())
        } else {
            if args.json {
                println!(
                    r#"{{"status":"error","message":"--fill requires either --coord <row,col> or --col <name/index>"}}"#
                );
            } else {
                eprintln!(
                    "Error: --fill requires either --coord <row,col> (e.g. --coord 5,age) or --col <name/index>"
                );
            }
            process::exit(1);
        };

        let output_path = if args.in_place {
            args.load.clone()
        } else if let Some(out) = &args.out {
            out.clone()
        } else {
            let stem = args
                .load
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("output");
            let ext = args
                .load
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("csv");
            let parent = args
                .load
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            parent.join(format!("{}_filled.{}", stem, ext))
        };

        let fill_config = FillConfig {
            input_path: args.load.clone(),
            output_path,
            in_place: args.in_place,
            target: FillTarget {
                row: target_row,
                col: target_col,
            },
            strategy,
        };

        match searchup::execute_fill(&fill_config) {
            Ok(result) => {
                if args.json {
                    match result.to_json(true) {
                        Ok(json_str) => println!("{}", json_str),
                        Err(e) => {
                            eprintln!("Error generating JSON: {}", e);
                            process::exit(1);
                        }
                    }
                } else {
                    print!("{}", format_fill_report(&result));
                }
            }
            Err(err) => {
                if args.json {
                    println!(r#"{{"status":"error","message":"{}"}}"#, err);
                } else {
                    eprintln!("Error: {}", err);
                }
                process::exit(1);
            }
        }
    } else if let Some(search_parts) = &args.search {
        // === Search / Analysis Mode ===
        if search_parts.is_empty() {
            eprintln!("Error: Please provide a search target (e.g. --search missing_values)");
            process::exit(1);
        }

        let search_query = search_parts.join(" ");
        let mode = SearchMode::parse(&search_query);

        let default_workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let workers = args.workers.unwrap_or(default_workers);

        let mut config = AnalysisConfig::new(args.load, mode, workers);
        config.batch_size = args.batch_size;
        config.limit = if args.all || args.limit == 0 {
            None
        } else {
            Some(args.limit)
        };

        match run_analysis(&config) {
            Ok(result) => {
                if args.json {
                    match result.to_json(true) {
                        Ok(json_output) => println!("{}", json_output),
                        Err(err) => {
                            eprintln!("Error generating JSON: {}", err);
                            process::exit(1);
                        }
                    }
                } else {
                    let report = format_report(&result);
                    print!("{}", report);
                }
            }
            Err(err) => {
                eprintln!("Error during analysis: {}", err);
                process::exit(1);
            }
        }
    } else {
        eprintln!(
            "Error: Please specify either --search <missing_values|pattern> or --fill ...\nUse --help for usage instructions."
        );
        process::exit(1);
    }
}
