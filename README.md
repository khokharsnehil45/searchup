# 🔍 SearchUp

**A high-performance, parallel CSV analyzer built with Rust.**

`searchup` scans large CSV files at blazing speeds to detect missing values, calculate data completeness metrics, and inspect datasets using multi-threaded parallel workers.

[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![Build Status](https://img.shields.io/badge/tests-passing-brightgreen.svg)]()

---

## ⚡ Features

- **Parallel Processing Engine**: Uses [Rayon](https://github.com/rayon-rs/rayon) to process CSV record batches concurrently across configurable worker threads.
- **Memory-Efficient Streaming**: Pipelines disk I/O and processing through a bounded batch channel. Safely analyzes multi-gigabyte files with low, predictable memory usage.
- **Automatic Dataset Profiling & Type Inference**: Automatically infers column data types (`Integer`, `Float`, `Boolean`, `DateTime`, `String`), counts nulls/uniques, and computes rich statistical summaries (`min`, `max`, `mean`, `median`, `std_dev`, IQR, top frequencies).
- **Outlier Detection via IQR**: Identifies numeric anomalies using Tukey's robust IQR rule with configurable worker parallelism (`--search outliers`).
- **Zero-Allocation Byte Scanning**: Inspects fields directly as raw byte slices, avoiding redundant UTF-8 string allocations for every cell.
- **Smart Missing Value Detection**: Detects empty cells, whitespace-only fields, omitted columns, and common missing value indicators (`NA`, `N/A`, `NULL`, `None`, `NaN`, `?`).
- **Missing Value Imputation Engine**: Repair missing cells with `--fill`, targeting coordinates `--coord 5,age` and applying strategies (`literal`, `mean`, `median`, `mode`).
- **Autonomous AI Agent Harness**: Autonomous reasoning engine (`--agent`) that inspects schemas, plans repair actions, calls tools, and imputes data autonomously via Ollama (local/free), OpenAI, Groq, or DeepSeek.
- **Detailed Terminal Reports & JSON**: Vibrant ANSI-colored terminal reports with progress bars, type highlights, and severity indicators (supports `--no-color` / `NO_COLOR`), or agent-ready structured JSON (`--json`).

---

## 🚀 Quick Start

### Installation

Clone the repository and install using Cargo:

```bash
git clone https://github.com/khokharsnehil45/searchup.git
cd searchup
cargo install --path .
```

Or build the release binary directly:

```bash
cargo build --release
# Executable will be in target/release/searchup
```

---

## 💻 Usage

### 1. Analyze Missing Values (Auto Workers)
By default, `searchup` uses all available logical CPU cores:

```bash
searchup --load data.csv --search missing values
```
*(You can also use `--search missing_values` or `--search "missing values"`)*

### 2. Specify Worker Count
Set the number of parallel workers using the `--workers` / `-w` flag:

```bash
searchup --load data.csv --search missing_values --workers 4
```

### 3. Search for Custom Text Patterns
Search across all columns for specific keywords or substrings:

```bash
searchup --load data.csv --search Canada --workers 4
```

### 4. Automatic Dataset Profiler (`--profile`)
Generate a comprehensive schema profile with inferred types (`Integer`, `Float`, `Boolean`, `DateTime`, `String`), null percentages, cardinalities, and statistical distributions:

```bash
# Formatted terminal profile report table
searchup --load data.csv --profile

# Full JSON schema & statistics for an AI agent
searchup --load data.csv --profile --json
```

### 5. Outlier Detection (`--search outliers`)
Detect numeric outliers across all numeric columns using Tukey's robust interquartile range (IQR) rule:

```bash
# Detect outliers with parallel workers
searchup --load data.csv --search outliers --workers 4

# Dump all outliers in structured JSON
searchup --load data.csv --search outliers --all --json
```

### 6. Structured JSON Output (For AI Agents & Automation)
Add `--json` to output machine-readable, structured JSON directly to stdout:

```bash
searchup --load data.csv --search missing_values --workers 4 --json
```

### 7. Dump ALL Missing Values (or Custom Limit)
By default, `searchup` aggregates all statistics across the entire file and previews the first 15 occurrences. To output **every single occurrence** without truncating:

```bash
# Dump all missing values (terminal or JSON)
searchup --load data.csv --search missing_values --all

# Combine with --json to get full row-by-row data for an agent
searchup --load data.csv --search missing_values --workers 4 --all --json

# Or set a custom occurrence limit
searchup --load data.csv --search missing_values --limit 100
```

### 8. Missing Value Imputation / Filling (`--fill`)
Clean and repair missing values manually or dynamically with statistical strategies:

```bash
# 1. Fill a specific coordinate manually with a literal value
searchup --load data.csv --fill --coord 5,age --val 25 --out cleaned.csv
# Also accepts numeric column index:
searchup --load data.csv --fill --coord "5, 3" --val 25 --out cleaned.csv

# 2. Fill a specific coordinate with the column's arithmetic mean
# (Fails safely if the column contains non-numeric text)
searchup --load data.csv --fill --coord 5,age --val mean --out cleaned.csv

# 3. Fill all missing values across an entire column with mean / median / mode
searchup --load data.csv --fill --col age --val mean --out cleaned.csv
searchup --load data.csv --fill --col country --val mode --out cleaned.csv

# 4. Safely update the file in-place (atomic temp-file replace)
searchup --load data.csv --fill --coord 5,age --val mean --in-place

# 5. Agent confirmation via structured JSON
searchup --load data.csv --fill --coord 5,score --val mean --out cleaned.csv --json
```

### 9. 🤖 Autonomous AI Agent Harness (`--agent`)
Run `searchup` with an autonomous reasoning harness that automatically inspects schemas, identifies missing data or outliers, and carries out data repair strategies based on natural language instructions:

```bash
# 1. Run a single instruction autonomously (OpenAI, DeepSeek, Groq, or local Ollama)
searchup --load data.csv --agent "Analyze missing values and impute age using mean"

# 2. Run completely offline and free with local Ollama!
searchup --load data.csv --agent "Profile this dataset and fix missing values" \
  --api-base http://localhost:11434/v1 --model llama3.1

# 3. Interactive Copilot Chat mode
searchup --load data.csv --agent
searchup> Check for outliers in salary
searchup> Impute missing department values with mode
searchup> exit
```

Supported API providers:
- **Local Ollama** (offline & free, default: `--api-base http://localhost:11434/v1 --model llama3.1`)
- **OpenAI** (`OPENAI_API_KEY`)
- **Groq** (`GROQ_API_KEY`, `--model llama-3.3-70b-versatile`)
- **DeepSeek** (`DEEPSEEK_API_KEY`, `--model deepseek-chat`)
- Any OpenAI-compatible tool calling endpoint.

---

## 📊 Sample Output

Running `searchup --load examples/sample.csv --search missing_values --workers 4`:

```text
======================================================================
                        SEARCHUP CSV ANALYZER                         
======================================================================
  File:            examples/sample.csv
  File Size:       284 bytes
  Search Target:   Missing Values
  Parallel Workers: 4
  Data Rows:       8
  Columns:         6
  Total Cells:     48
----------------------------------------------------------------------
  Total Missing Values: 8 (16.67% of all cells)
  Rows Affected:       7 (87.50% of data rows)
----------------------------------------------------------------------
  Breakdown by Column (Missing Values):
    #    Column Name   Missing Values Percentage  Distribution
    1    id                         0      0.00%  [               ]
    2    name                       1     12.50%  [==             ]
    3    age                        2     25.00%  [====           ]
    4    email                      2     25.00%  [====           ]
    5    country                    2     25.00%  [====           ]
    6    score                      1     12.50%  [==             ]
----------------------------------------------------------------------
  Sample Occurrences (showing up to 8):
    - Row 2      | Column 2  ("name") -> <empty>
    - Row 2      | Column 6  ("score") -> <empty>
    - Row 3      | Column 3  ("age") -> NA
    - Row 3      | Column 4  ("email") -> <empty>
    - Row 4      | Column 5  ("country") -> null
    - Row 5      | Column 3  ("age") -> <empty>
    - Row 5      | Column 6  ("score") -> None
    - Row 6      | Column 4  ("email") -> <empty>
======================================================================
  Execution Time:  1.12ms (7142.9 rows/sec, 42857.1 cells/sec)
======================================================================
```

### Structured JSON Format (`--json`)

```json
{
  "file_path": "sample.csv",
  "file_size_bytes": 284,
  "search_target": "missing_values",
  "workers_used": 4,
  "execution_time_ms": 1.12,
  "summary": {
    "total_rows": 8,
    "total_columns": 6,
    "total_cells": 48,
    "total_matches": 8,
    "match_percentage_cells": 16.67,
    "affected_rows": 7,
    "affected_percentage_rows": 87.5
  },
  "columns": [
    {
      "index": 1,
      "name": "id",
      "match_count": 0,
      "match_percentage": 0.0,
      "total_rows": 8
    },
    {
      "index": 2,
      "name": "name",
      "match_count": 1,
      "match_percentage": 12.5,
      "total_rows": 8
    }
  ],
  "sample_occurrences": [
    {
      "row": 2,
      "col_idx": 1,
      "col_name": "name",
      "value_preview": "<empty>"
    }
  ]
}
```

---

## 🛠️ CLI Options

```text
A fast CLI tool that utilizes parallel worker threads to analyze CSV files, detect missing values, and inspect data completeness.

Usage: searchup [OPTIONS] --load <FILE>

Options:
  -l, --load <FILE>              Path to the CSV file to analyze or modify
  -s, --search <TARGET>...       Target to search (e.g. 'missing_values', 'outliers', or text query)
      --profile                  Generate comprehensive data profile with type inference and column stats
  -w, --workers <NUM>            Number of parallel worker threads (defaults to available CPU cores)
      --batch-size <BATCH_SIZE>  Batch size for worker chunks [default: 4096]
      --json                     Dump structured JSON output for AI agents and automation
      --limit <LIMIT>            Maximum occurrences to output in detail (default: 15; use 0 for unlimited)
  -a, --all                      Output ALL occurrences without truncating

Fill / Imputation Options:
      --fill                     Enable missing value fill / imputation mode
      --coord <ROW,COL>          Coordinate to fill, e.g. '5,3', '5,age', or '(5,3)'
      --row <ROW>                Target row number (1-based line number)
      --col <COL>                Target column name or 1-based index
      --val <VALUE>              Imputed value or strategy ('mean', 'median', 'mode', or literal)
  -o, --out <FILE>               Destination file to write modified CSV
      --in-place                 Overwrite loaded CSV in-place safely via atomic temporary file
      --no-color                 Disable colored terminal output

Agent Harness Options:
      --agent [<PROMPT>]         Run autonomous agent mode with prompt (or interactive mode)
      --model <MODEL>            Model name [default: gpt-4o-mini]
      --api-base <URL>           API Base URL (e.g. 'http://localhost:11434/v1' for Ollama)
      --api-key <KEY>            API key (or use OPENAI_API_KEY / GROQ_API_KEY)
  -h, --help                     Print help
  -V, --version                  Print version
```

---

## 🧪 Running Tests

The test suite covers unit tests, missing value accuracy, text searches, and multi-worker consistency:

```bash
cargo test
```

---

## 📄 License

Dual-licensed under either:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
