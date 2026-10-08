use rayon::prelude::*;
use serde::Serialize;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum SearchMode {
    MissingValues,
    Text(String),
}

impl SearchMode {
    pub fn parse(input: &str) -> Self {
        let normalized = input.trim().to_lowercase().replace(['-', ' '], "_");
        if normalized == "missing_values" || normalized == "missing" || normalized == "missing_value" {
            SearchMode::MissingValues
        } else {
            SearchMode::Text(input.trim().to_string())
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            SearchMode::MissingValues => "Missing Values".to_string(),
            SearchMode::Text(pattern) => format!("Text Pattern: \"{pattern}\""),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnalysisConfig {
    pub file_path: PathBuf,
    pub mode: SearchMode,
    pub workers: usize,
    pub batch_size: usize,
}

impl AnalysisConfig {
    pub fn new(file_path: PathBuf, mode: SearchMode, workers: usize) -> Self {
        Self {
            file_path,
            mode,
            workers: workers.max(1),
            batch_size: 4096,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Occurrence {
    pub row: usize,
    pub col_idx: usize,
    pub col_name: String,
    pub value_preview: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnStats {
    pub index: usize,
    pub name: String,
    pub match_count: usize,
    pub total_rows: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSummary {
    pub total_rows: usize,
    pub total_columns: usize,
    pub total_cells: usize,
    pub total_matches: usize,
    pub match_percentage_cells: f64,
    pub affected_rows: usize,
    pub affected_percentage_rows: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentColumnInfo {
    pub index: usize,
    pub name: String,
    pub match_count: usize,
    pub match_percentage: f64,
    pub total_rows: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentReport {
    pub file_path: String,
    pub file_size_bytes: u64,
    pub search_target: String,
    pub workers_used: usize,
    pub execution_time_ms: f64,
    pub summary: AgentSummary,
    pub columns: Vec<AgentColumnInfo>,
    pub sample_occurrences: Vec<Occurrence>,
}

#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub file_path: PathBuf,
    pub file_size_bytes: u64,
    pub search_mode: SearchMode,
    pub workers_used: usize,
    pub duration: Duration,
    pub total_rows: usize,
    pub total_columns: usize,
    pub total_cells: usize,
    pub affected_rows: usize,
    pub total_matches: usize,
    pub column_stats: Vec<ColumnStats>,
    pub sample_occurrences: Vec<Occurrence>,
}

impl AnalysisResult {
    pub fn to_agent_report(&self) -> AgentReport {
        let cell_pct = if self.total_cells > 0 {
            (self.total_matches as f64 / self.total_cells as f64) * 100.0
        } else {
            0.0
        };
        let row_pct = if self.total_rows > 0 {
            (self.affected_rows as f64 / self.total_rows as f64) * 100.0
        } else {
            0.0
        };

        let columns = self
            .column_stats
            .iter()
            .map(|c| {
                let pct = if c.total_rows > 0 {
                    (c.match_count as f64 / c.total_rows as f64) * 100.0
                } else {
                    0.0
                };
                AgentColumnInfo {
                    index: c.index,
                    name: c.name.clone(),
                    match_count: c.match_count,
                    match_percentage: (pct * 100.0).round() / 100.0,
                    total_rows: c.total_rows,
                }
            })
            .collect();

        AgentReport {
            file_path: self.file_path.display().to_string(),
            file_size_bytes: self.file_size_bytes,
            search_target: match &self.search_mode {
                SearchMode::MissingValues => "missing_values".to_string(),
                SearchMode::Text(query) => query.clone(),
            },
            workers_used: self.workers_used,
            execution_time_ms: (self.duration.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
            summary: AgentSummary {
                total_rows: self.total_rows,
                total_columns: self.total_columns,
                total_cells: self.total_cells,
                total_matches: self.total_matches,
                match_percentage_cells: (cell_pct * 100.0).round() / 100.0,
                affected_rows: self.affected_rows,
                affected_percentage_rows: (row_pct * 100.0).round() / 100.0,
            },
            columns,
            sample_occurrences: self.sample_occurrences.clone(),
        }
    }

    pub fn to_json(&self, pretty: bool) -> Result<String, serde_json::Error> {
        let report = self.to_agent_report();
        if pretty {
            serde_json::to_string_pretty(&report)
        } else {
            serde_json::to_string(&report)
        }
    }
}

pub fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while let Some((first, rest)) = bytes.split_first() {
        if first.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    while let Some((last, rest)) = bytes.split_last() {
        if last.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}

pub fn is_missing_value(bytes: &[u8]) -> bool {
    let trimmed = trim_ascii_whitespace(bytes);
    if trimmed.is_empty() {
        return true;
    }
    trimmed.eq_ignore_ascii_case(b"na")
        || trimmed.eq_ignore_ascii_case(b"n/a")
        || trimmed.eq_ignore_ascii_case(b"null")
        || trimmed.eq_ignore_ascii_case(b"none")
        || trimmed.eq_ignore_ascii_case(b"nan")
        || trimmed.eq_ignore_ascii_case(b"?")
}

pub fn field_matches(bytes: &[u8], mode: &SearchMode) -> bool {
    match mode {
        SearchMode::MissingValues => is_missing_value(bytes),
        SearchMode::Text(query) => {
            if let Ok(s) = std::str::from_utf8(bytes) {
                s.to_lowercase().contains(&query.to_lowercase())
            } else {
                let lower_bytes = bytes.to_ascii_lowercase();
                let query_bytes = query.to_ascii_lowercase();
                lower_bytes.windows(query_bytes.len()).any(|w| w == query_bytes.as_bytes())
            }
        }
    }
}

#[derive(Default)]
struct BatchStats {
    row_count: usize,
    rows_affected: usize,
    total_matches: usize,
    col_counts: Vec<usize>,
    samples: Vec<Occurrence>,
    error: Option<String>,
}

impl BatchStats {
    fn combine(mut self, mut other: Self) -> Self {
        if self.error.is_none() {
            self.error = other.error.take();
        }
        self.row_count += other.row_count;
        self.rows_affected += other.rows_affected;
        self.total_matches += other.total_matches;

        if other.col_counts.len() > self.col_counts.len() {
            self.col_counts.resize(other.col_counts.len(), 0);
        }
        for (i, count) in other.col_counts.into_iter().enumerate() {
            self.col_counts[i] += count;
        }

        self.samples.extend(other.samples);
        self.samples.sort_by_key(|s| (s.row, s.col_idx));
        self.samples.truncate(15);
        self
    }
}

pub fn run_analysis(config: &AnalysisConfig) -> Result<AnalysisResult, Box<dyn std::error::Error + Send + Sync>> {
    let start_time = Instant::now();
    let file = File::open(&config.file_path)?;
    let metadata = file.metadata()?;
    let file_size_bytes = metadata.len();

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(BufReader::with_capacity(128 * 1024, file));

    let header_record = reader.byte_headers()?.clone();
    let headers: Vec<String> = if header_record.is_empty() {
        Vec::new()
    } else {
        header_record
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let name = String::from_utf8_lossy(h).trim().to_string();
                if name.is_empty() {
                    format!("Column_{}", i + 1)
                } else {
                    name
                }
            })
            .collect()
    };

    let col_count = headers.len();
    let batch_size = config.batch_size;
    let (tx, rx) = sync_channel::<Result<Vec<(usize, csv::ByteRecord)>, String>>(16);

    let producer = std::thread::spawn(move || {
        let mut current_batch = Vec::with_capacity(batch_size);
        let mut row_idx = 2; // header is row 1, data starts at row 2
        let mut record = csv::ByteRecord::new();

        loop {
            match reader.read_byte_record(&mut record) {
                Ok(true) => {
                    current_batch.push((row_idx, record.clone()));
                    row_idx += 1;
                    if current_batch.len() >= batch_size {
                        if tx.send(Ok(current_batch)).is_err() {
                            return;
                        }
                        current_batch = Vec::with_capacity(batch_size);
                    }
                }
                Ok(false) => {
                    if !current_batch.is_empty() {
                        let _ = tx.send(Ok(current_batch));
                    }
                    break;
                }
                Err(err) => {
                    let _ = tx.send(Err(format!("CSV read error at row {row_idx}: {err}")));
                    return;
                }
            }
        }
    });

    let thread_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.workers)
        .build()?;

    let mode = config.mode.clone();
    let headers_arc = std::sync::Arc::new(headers.clone());

    let final_stats = thread_pool.install(|| {
        rx.into_iter()
            .par_bridge()
            .map(|batch_res| match batch_res {
                Ok(batch) => {
                    let mut stats = BatchStats {
                        row_count: batch.len(),
                        rows_affected: 0,
                        total_matches: 0,
                        col_counts: vec![0; col_count],
                        samples: Vec::new(),
                        error: None,
                    };

                    for (row_idx, record) in batch {
                        let max_cols = col_count.max(record.len());
                        if max_cols > stats.col_counts.len() {
                            stats.col_counts.resize(max_cols, 0);
                        }

                        let mut row_has_match = false;
                        for i in 0..max_cols {
                            let (matched, preview) = if i < record.len() {
                                let field = &record[i];
                                if field_matches(field, &mode) {
                                    let s = String::from_utf8_lossy(field).trim().to_string();
                                    let prev = if s.is_empty() { "<empty>".to_string() } else { s };
                                    (true, prev)
                                } else {
                                    (false, String::new())
                                }
                            } else {
                                // Column exists in header but not in this row -> missing value
                                if matches!(mode, SearchMode::MissingValues) {
                                    (true, "<omitted>".to_string())
                                } else {
                                    (false, String::new())
                                }
                            };

                            if matched {
                                row_has_match = true;
                                stats.total_matches += 1;
                                stats.col_counts[i] += 1;

                                if stats.samples.len() < 15 {
                                    let col_name = if i < headers_arc.len() {
                                        headers_arc[i].clone()
                                    } else {
                                        format!("Column_{}", i + 1)
                                    };
                                    stats.samples.push(Occurrence {
                                        row: row_idx,
                                        col_idx: i,
                                        col_name,
                                        value_preview: preview,
                                    });
                                }
                            }
                        }

                        if row_has_match {
                            stats.rows_affected += 1;
                        }
                    }

                    stats
                }
                Err(err) => BatchStats {
                    error: Some(err),
                    ..Default::default()
                },
            })
            .reduce(BatchStats::default, BatchStats::combine)
    });

    if let Err(e) = producer.join() {
        return Err(format!("Producer thread panicked: {:?}", e).into());
    }

    if let Some(err_msg) = final_stats.error {
        return Err(err_msg.into());
    }

    let duration = start_time.elapsed();
    let total_rows = final_stats.row_count;
    let actual_col_count = col_count.max(final_stats.col_counts.len());
    let total_cells = total_rows * actual_col_count;

    let mut column_stats = Vec::with_capacity(actual_col_count);
    for i in 0..actual_col_count {
        let name = if i < headers.len() {
            headers[i].clone()
        } else {
            format!("Column_{}", i + 1)
        };
        let match_count = if i < final_stats.col_counts.len() {
            final_stats.col_counts[i]
        } else {
            0
        };
        column_stats.push(ColumnStats {
            index: i + 1,
            name,
            match_count,
            total_rows,
        });
    }

    Ok(AnalysisResult {
        file_path: config.file_path.clone(),
        file_size_bytes,
        search_mode: config.mode.clone(),
        workers_used: config.workers,
        duration,
        total_rows,
        total_columns: actual_col_count,
        total_cells,
        affected_rows: final_stats.rows_affected,
        total_matches: final_stats.total_matches,
        column_stats,
        sample_occurrences: final_stats.samples,
    })
}

pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

pub fn format_report(result: &AnalysisResult) -> String {
    let mut out = String::new();
    let is_missing = matches!(result.search_mode, SearchMode::MissingValues);
    let target_label = if is_missing { "Missing Values" } else { "Matches" };

    out.push_str("======================================================================\n");
    out.push_str("                        SEARCHUP CSV ANALYZER                         \n");
    out.push_str("======================================================================\n");
    out.push_str(&format!("  File:            {}\n", result.file_path.display()));
    out.push_str(&format!("  File Size:       {}\n", format_size(result.file_size_bytes)));
    out.push_str(&format!("  Search Target:   {}\n", result.search_mode.display_name()));
    out.push_str(&format!("  Parallel Workers: {}\n", result.workers_used));
    out.push_str(&format!("  Data Rows:       {}\n", format_number(result.total_rows)));
    out.push_str(&format!("  Columns:         {}\n", result.total_columns));
    out.push_str(&format!("  Total Cells:     {}\n", format_number(result.total_cells)));
    out.push_str("----------------------------------------------------------------------\n");

    let cell_pct = if result.total_cells > 0 {
        (result.total_matches as f64 / result.total_cells as f64) * 100.0
    } else {
        0.0
    };
    let row_pct = if result.total_rows > 0 {
        (result.affected_rows as f64 / result.total_rows as f64) * 100.0
    } else {
        0.0
    };

    out.push_str(&format!(
        "  Total {}: {} ({:.2}% of all cells)\n",
        target_label,
        format_number(result.total_matches),
        cell_pct
    ));
    out.push_str(&format!(
        "  Rows Affected:       {} ({:.2}% of data rows)\n",
        format_number(result.affected_rows),
        row_pct
    ));
    out.push_str("----------------------------------------------------------------------\n");
    out.push_str(&format!("  Breakdown by Column ({}):\n", target_label));

    // Find max column name length for clean alignment
    let max_name_len = result
        .column_stats
        .iter()
        .map(|c| c.name.len())
        .max()
        .unwrap_or(10)
        .max(11)
        .min(30);

    out.push_str(&format!(
        "    {:<4} {:<width$} {:>12} {:>10}  {}\n",
        "#",
        "Column Name",
        target_label,
        "Percentage",
        "Distribution",
        width = max_name_len
    ));

    for col in &result.column_stats {
        let col_pct = if col.total_rows > 0 {
            (col.match_count as f64 / col.total_rows as f64) * 100.0
        } else {
            0.0
        };
        let bar = generate_progress_bar(col_pct, 15);
        let display_name = if col.name.len() > max_name_len {
            format!("{}...", &col.name[..max_name_len - 3])
        } else {
            col.name.clone()
        };

        out.push_str(&format!(
            "    {:<4} {:<width$} {:>12} {:>9.2}%  [{}]\n",
            col.index,
            display_name,
            format_number(col.match_count),
            col_pct,
            bar,
            width = max_name_len
        ));
    }

    if !result.sample_occurrences.is_empty() {
        out.push_str("----------------------------------------------------------------------\n");
        out.push_str(&format!(
            "  Sample Occurrences (showing up to {}):\n",
            result.sample_occurrences.len()
        ));
        for occ in &result.sample_occurrences {
            out.push_str(&format!(
                "    - Row {:<6} | Column {:<2} (\"{}\") -> {}\n",
                occ.row,
                occ.col_idx + 1,
                occ.col_name,
                occ.value_preview
            ));
        }
    }

    let elapsed_sec = result.duration.as_secs_f64();
    let rows_per_sec = if elapsed_sec > 0.0 {
        result.total_rows as f64 / elapsed_sec
    } else {
        0.0
    };
    let cells_per_sec = if elapsed_sec > 0.0 {
        result.total_cells as f64 / elapsed_sec
    } else {
        0.0
    };

    out.push_str("======================================================================\n");
    out.push_str(&format!(
        "  Execution Time:  {:.2?} ({:.1} rows/sec, {:.1} cells/sec)\n",
        result.duration, rows_per_sec, cells_per_sec
    ));
    out.push_str("======================================================================\n");

    out
}

fn generate_progress_bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width.saturating_sub(filled);
    format!("{}{}", "=".repeat(filled), " ".repeat(empty))
}

fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let mut count = 0;
    for c in s.chars().rev() {
        if count > 0 && count % 3 == 0 {
            result.push(',');
        }
        result.push(c);
        count += 1;
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_missing_value_detection() {
        assert!(is_missing_value(b""));
        assert!(is_missing_value(b"   "));
        assert!(is_missing_value(b"\t\r\n"));
        assert!(is_missing_value(b"na"));
        assert!(is_missing_value(b"NA"));
        assert!(is_missing_value(b"n/a"));
        assert!(is_missing_value(b"N/A"));
        assert!(is_missing_value(b"null"));
        assert!(is_missing_value(b"NULL"));
        assert!(is_missing_value(b"none"));
        assert!(is_missing_value(b"None"));
        assert!(is_missing_value(b"nan"));
        assert!(is_missing_value(b"NaN"));
        assert!(is_missing_value(b"?"));

        assert!(!is_missing_value(b"0"));
        assert!(!is_missing_value(b"false"));
        assert!(!is_missing_value(b"hello world"));
        assert!(!is_missing_value(b"123.45"));
    }

    #[test]
    fn test_search_mode_parsing() {
        assert_eq!(SearchMode::parse("missing values"), SearchMode::MissingValues);
        assert_eq!(SearchMode::parse("missing_values"), SearchMode::MissingValues);
        assert_eq!(SearchMode::parse("missing-values"), SearchMode::MissingValues);
        assert_eq!(SearchMode::parse("MISSING"), SearchMode::MissingValues);
        assert_eq!(SearchMode::parse("apple"), SearchMode::Text("apple".to_string()));
    }

    #[test]
    fn test_number_formatting() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1234567), "1,234,567");
    }

    #[test]
    fn test_json_serialization() {
        let result = AnalysisResult {
            file_path: PathBuf::from("test.csv"),
            file_size_bytes: 1024,
            search_mode: SearchMode::MissingValues,
            workers_used: 4,
            duration: Duration::from_millis(50),
            total_rows: 10,
            total_columns: 2,
            total_cells: 20,
            affected_rows: 2,
            total_matches: 3,
            column_stats: vec![
                ColumnStats {
                    index: 1,
                    name: "col1".to_string(),
                    match_count: 1,
                    total_rows: 10,
                },
                ColumnStats {
                    index: 2,
                    name: "col2".to_string(),
                    match_count: 2,
                    total_rows: 10,
                },
            ],
            sample_occurrences: vec![Occurrence {
                row: 2,
                col_idx: 0,
                col_name: "col1".to_string(),
                value_preview: "<empty>".to_string(),
            }],
        };

        let json = result.to_json(true).expect("JSON serialization failed");
        assert!(json.contains("\"search_target\": \"missing_values\""));
        assert!(json.contains("\"total_missing\": 3") || json.contains("\"total_matches\": 3"));
        assert!(json.contains("\"file_path\": \"test.csv\""));
        assert!(json.contains("\"workers_used\": 4"));
    }
}
