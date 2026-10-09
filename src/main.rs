use clap::Parser;
use colored::Colorize;
use searchup::{
    format_fill_report, format_profile_report, format_report, parse_coord, run_analysis,
    AnalysisConfig, FillConfig, FillStrategy, FillTarget, SearchMode,
};
use std::path::PathBuf;
use std::process;

#[derive(Parser, Debug)]
#[command(
    name = "searchup",
    author = "SearchUp",
    version,
    about = "High-performance parallel CSV analyzer, profiler, and imputer",
    long_about = "A fast CLI tool that utilizes parallel worker threads to analyze CSV files, detect missing values, find outliers, profile schemas, and fill/impute data completeness for AI agents."
)]
struct Args {
    /// Path to the CSV file to analyze or modify
    #[arg(short, long, value_name = "FILE")]
    load: PathBuf,

    /// Target to search (e.g. 'missing_values', 'outliers', or text query)
    #[arg(short, long, num_args(1..), value_name = "TARGET")]
    search: Option<Vec<String>>,

    /// Generate a comprehensive data profile with type inference and column stats
    #[arg(long)]
    profile: bool,

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

    /// Disable colored terminal output
    #[arg(long)]
    no_color: bool,

    // === Agent Harness Arguments ===

    /// Run autonomous agent mode with optional prompt instruction (or enter interactive mode)
    #[arg(long, num_args(0..=1), value_name = "PROMPT")]
    agent: Option<Option<String>>,

    /// Model to use for the agent (default: 'gpt-4o-mini', or 'llama3.1', 'deepseek-chat')
    #[arg(long, default_value = "gpt-4o-mini", value_name = "MODEL")]
    model: String,

    /// API Base URL (e.g. 'https://api.openai.com/v1' or 'http://localhost:11434/v1')
    #[arg(long, value_name = "URL")]
    api_base: Option<String>,

    /// API key for the LLM provider (or use OPENAI_API_KEY environment variable)
    #[arg(long, value_name = "KEY")]
    api_key: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    if args.no_color {
        colored::control::set_override(false);
    }

    if let Some(w) = args.workers {
        if w == 0 {
            eprintln!("{}: --workers must be greater than 0", "Error".bright_red().bold());
            process::exit(1);
        }
    }

    if !args.load.exists() {
        eprintln!("{}: File '{}' does not exist.", "Error".bright_red().bold(), args.load.display());
        process::exit(1);
    }

    // Check if agent mode was requested
    let is_agent = args.agent.is_some()
        || args.search.as_ref().map_or(false, |s| {
            let q = s.first().map(|x| x.to_lowercase()).unwrap_or_default();
            q == "agent" || q == "chat"
        });

    if is_agent {
        let default_workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let workers = args.workers.unwrap_or(default_workers);

        let api_key = args
            .api_key
            .or_else(|| std::env::var("NVIDIA_API_KEY").ok())
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
            .or_else(|| std::env::var("GROQ_API_KEY").ok())
            .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok());

        let is_nvidia = api_key
            .as_ref()
            .map_or(false, |k| k.starts_with("nvapi-"))
            || std::env::var("NVIDIA_API_KEY").is_ok();

        let api_base = if let Some(base) = args.api_base {
            base
        } else if let Ok(base) = std::env::var("OPENAI_BASE_URL") {
            base
        } else if is_nvidia {
            "https://integrate.api.nvidia.com/v1".to_string()
        } else {
            "https://api.openai.com/v1".to_string()
        };

        let model = if args.model == "gpt-4o-mini" && is_nvidia {
            "meta/llama-3.3-70b-instruct".to_string()
        } else {
            args.model
        };

        if api_key.is_none() && api_base.contains("api.openai.com") {
            println!(
                "  {} {}\n  {}\n",
                "ℹ Note:".bright_yellow().bold(),
                "No API key detected for OpenAI.".white(),
                "Tip: Set NVIDIA_API_KEY, OPENAI_API_KEY, GROQ_API_KEY, DEEPSEEK_API_KEY, or run with local Ollama:\n    searchup --load <file> --agent --api-base http://localhost:11434/v1 --model llama3.1".dimmed()
            );
        }

        let prompt = if let Some(Some(p)) = &args.agent {
            Some(p.clone())
        } else if let Some(search_parts) = &args.search {
            if search_parts.len() > 1 {
                Some(search_parts[1..].join(" "))
            } else {
                None
            }
        } else {
            None
        };

        let agent_config = searchup::agent::AgentConfig {
            file_path: args.load,
            api_key,
            api_base,
            model,
            max_turns: 12,
            workers,
        };

        if let Err(e) = searchup::agent::run_agent_session(agent_config, prompt).await {
            eprintln!("{}: {}", "Agent error".bright_red().bold(), e);
            process::exit(1);
        }
        return;
    }

    // Check if profiling mode was requested
    let is_profile = args.profile
        || args.search.as_ref().map_or(false, |s| {
            let q = s.join(" ").to_lowercase();
            q == "profile" || q == "profiling" || q == "summary"
        });

    if is_profile {
        // === Data Profiling Mode ===
        let default_workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let workers = args.workers.unwrap_or(default_workers);

        let mut config = AnalysisConfig::new(args.load, SearchMode::MissingValues, workers);
        config.batch_size = args.batch_size;

        match searchup::run_profile(&config) {
            Ok(report) => {
                if args.json {
                    println!("{}", report.to_json(true).unwrap());
                } else {
                    print!("{}", format_profile_report(&report));
                }
            }
            Err(err) => {
                if args.json {
                    println!(r#"{{"status":"error","message":"{}"}}"#, err);
                } else {
                    eprintln!("{}: {}", "Error during profiling".bright_red().bold(), err);
                }
                process::exit(1);
            }
        }
        return;
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
                    eprintln!("{}: --fill requires --val <value|mean|median|mode>", "Error".bright_red().bold());
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
                        eprintln!("{}: {}", "Error".bright_red().bold(), err);
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
                    "{}: --fill requires either --coord <row,col> (e.g. --coord 5,age) or --col <name/index>",
                    "Error".bright_red().bold()
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
                    eprintln!("{}: {}", "Error".bright_red().bold(), err);
                }
                process::exit(1);
            }
        }
    } else if let Some(search_parts) = &args.search {
        // === Search / Analysis Mode ===
        if search_parts.is_empty() {
            eprintln!("{}: Please provide a search target (e.g. --search missing_values)", "Error".bright_red().bold());
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
                            eprintln!("{}: {}", "Error generating JSON".bright_red().bold(), err);
                            process::exit(1);
                        }
                    }
                } else {
                    let report = format_report(&result);
                    print!("{}", report);
                }
            }
            Err(err) => {
                eprintln!("{}: {}", "Error during analysis".bright_red().bold(), err);
                process::exit(1);
            }
        }
    } else {
        eprintln!(
            "{}: Please specify --profile, --search <missing_values|outliers|pattern>, or --fill ...\nUse --help for usage instructions.",
            "Error".bright_red().bold()
        );
        process::exit(1);
    }
}
