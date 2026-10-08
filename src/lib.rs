use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
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
    pub limit: Option<usize>,
}

impl AnalysisConfig {
    pub fn new(file_path: PathBuf, mode: SearchMode, workers: usize) -> Self {
        Self {
            file_path,
            mode,
            workers: workers.max(1),
            batch_size: 4096,
            limit: Some(15),
        }
    }

    pub fn with_limit(mut self, limit: Option<usize>) -> Self {
        self.limit = limit;
        self
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
    pub occurrences_count: usize,
    pub occurrences: Vec<Occurrence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_occurrences: Option<Vec<Occurrence>>,
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
            occurrences_count: self.sample_occurrences.len(),
            occurrences: self.sample_occurrences.clone(),
            sample_occurrences: Some(self.sample_occurrences.clone()),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum FillStrategy {
    Literal(String),
    Mean,
    Median,
    Mode,
}

impl FillStrategy {
    pub fn parse(input: &str) -> Self {
        match input.trim().to_lowercase().as_str() {
            "mean" => FillStrategy::Mean,
            "median" => FillStrategy::Median,
            "mode" => FillStrategy::Mode,
            _ => FillStrategy::Literal(input.trim().to_string()),
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            FillStrategy::Literal(v) => format!("Literal: \"{}\"", v),
            FillStrategy::Mean => "Mean (arithmetic average)".to_string(),
            FillStrategy::Median => "Median".to_string(),
            FillStrategy::Mode => "Mode (most frequent)".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FillTarget {
    pub row: Option<usize>, // 1-based CSV line number (matching occurrence row index)
    pub col: String,        // Column name or 1-based index
}

pub fn parse_coord(coord_str: &str) -> Result<(usize, String), String> {
    let clean = coord_str
        .trim()
        .trim_matches(|c| c == '(' || c == ')' || c == '[' || c == ']');
    let parts: Vec<&str> = clean.split(',').map(|s| s.trim()).collect();
    if parts.len() != 2 {
        return Err(format!(
            "Invalid coordinate '{}'. Expected 'row,col', e.g. '5,age' or '5,3'",
            coord_str
        ));
    }
    let row: usize = parts[0]
        .parse()
        .map_err(|_| format!("Invalid row index '{}' in coordinate", parts[0]))?;
    if row < 2 {
        return Err(format!(
            "Invalid row {}: header is row 1, data rows start at row 2",
            row
        ));
    }
    let col = parts[1].to_string();
    if col.is_empty() {
        return Err("Column identifier cannot be empty".to_string());
    }
    Ok((row, col))
}

#[derive(Debug, Clone)]
pub struct FillConfig {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub in_place: bool,
    pub target: FillTarget,
    pub strategy: FillStrategy,
}

#[derive(Debug, Clone, Serialize)]
pub struct FillResult {
    pub status: String,
    pub action: String,
    pub file_path: String,
    pub output_path: String,
    pub target_row: Option<usize>,
    pub target_column: String,
    pub target_column_index: usize,
    pub strategy: String,
    pub imputed_value: String,
    pub cells_updated: usize,
}

impl FillResult {
    pub fn to_json(&self, pretty: bool) -> Result<String, serde_json::Error> {
        if pretty {
            serde_json::to_string_pretty(self)
        } else {
            serde_json::to_string(self)
        }
    }
}

pub fn execute_fill(
    config: &FillConfig,
) -> Result<FillResult, Box<dyn std::error::Error + Send + Sync>> {
    let file = File::open(&config.input_path)?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(BufReader::with_capacity(128 * 1024, file));

    let headers: Vec<String> = reader
        .headers()?
        .iter()
        .map(|h| h.trim().to_string())
        .collect();

    if headers.is_empty() {
        return Err("CSV file contains no headers".into());
    }

    // Resolve column index
    let (col_idx, col_name) = if let Ok(one_based_idx) = config.target.col.parse::<usize>() {
        if one_based_idx == 0 || one_based_idx > headers.len() {
            return Err(format!(
                "Column index {} out of range (1 to {})",
                one_based_idx,
                headers.len()
            )
            .into());
        }
        let zero_idx = one_based_idx - 1;
        (zero_idx, headers[zero_idx].clone())
    } else {
        let needle = config.target.col.trim().to_lowercase();
        let found = headers
            .iter()
            .enumerate()
            .find(|(_, h)| h.trim().to_lowercase() == needle);
        match found {
            Some((idx, name)) => (idx, name.clone()),
            None => {
                return Err(format!(
                    "Column '{}' not found in CSV headers: {:?}",
                    config.target.col, headers
                )
                .into());
            }
        }
    };

    // Determine imputed value based on strategy
    let imputed_value = match &config.strategy {
        FillStrategy::Literal(val) => val.clone(),
        FillStrategy::Mean | FillStrategy::Median => {
            let file_scan = File::open(&config.input_path)?;
            let mut scan_reader = csv::ReaderBuilder::new()
                .has_headers(true)
                .flexible(true)
                .from_reader(BufReader::new(file_scan));

            let mut numbers: Vec<f64> = Vec::new();
            let mut row_num = 2;
            let mut record = csv::ByteRecord::new();

            while scan_reader.read_byte_record(&mut record)? {
                if col_idx < record.len() {
                    let field = &record[col_idx];
                    if !is_missing_value(field) {
                        let text = std::str::from_utf8(field).map_err(|_| {
                            format!(
                                "Column '{}' at row {} contains non-UTF8 bytes; cannot compute {}",
                                col_name,
                                row_num,
                                config.strategy.display_name()
                            )
                        })?;
                        let trimmed = text.trim();
                        match trimmed.parse::<f64>() {
                            Ok(num) => numbers.push(num),
                            Err(_) => {
                                return Err(format!(
                                    "Column '{}' contains non-numeric value \"{}\" at row {}; cannot compute {}",
                                    col_name,
                                    trimmed,
                                    row_num,
                                    config.strategy.display_name()
                                )
                                .into());
                            }
                        }
                    }
                }
                row_num += 1;
            }

            if numbers.is_empty() {
                return Err(format!(
                    "Column '{}' contains no non-missing numeric values to compute {}",
                    col_name,
                    config.strategy.display_name()
                )
                .into());
            }

            match config.strategy {
                FillStrategy::Mean => {
                    let sum: f64 = numbers.iter().sum();
                    let mean = sum / numbers.len() as f64;
                    if mean.fract() == 0.0 {
                        format!("{:.0}", mean)
                    } else {
                        format!("{:.2}", mean)
                    }
                }
                FillStrategy::Median => {
                    numbers.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    let mid = numbers.len() / 2;
                    let median = if numbers.len() % 2 == 0 {
                        (numbers[mid - 1] + numbers[mid]) / 2.0
                    } else {
                        numbers[mid]
                    };
                    if median.fract() == 0.0 {
                        format!("{:.0}", median)
                    } else {
                        format!("{:.2}", median)
                    }
                }
                _ => unreachable!(),
            }
        }
        FillStrategy::Mode => {
            let file_scan = File::open(&config.input_path)?;
            let mut scan_reader = csv::ReaderBuilder::new()
                .has_headers(true)
                .flexible(true)
                .from_reader(BufReader::new(file_scan));

            let mut freq: HashMap<String, usize> = HashMap::new();
            let mut record = csv::ByteRecord::new();

            while scan_reader.read_byte_record(&mut record)? {
                if col_idx < record.len() {
                    let field = &record[col_idx];
                    if !is_missing_value(field) {
                        let text = String::from_utf8_lossy(field).trim().to_string();
                        *freq.entry(text).or_insert(0) += 1;
                    }
                }
            }

            let mode_val = freq
                .into_iter()
                .max_by_key(|&(_, count)| count)
                .map(|(val, _)| val)
                .ok_or_else(|| {
                    format!(
                        "Column '{}' contains no non-missing values to compute mode",
                        col_name
                    )
                })?;
            mode_val
        }
    };

    // Second pass: write modified CSV
    let target_write_path = if config.in_place {
        let parent = config.input_path.parent().unwrap_or_else(|| Path::new("."));
        let temp_filename = format!(
            ".searchup_tmp_{}_{}.csv",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        parent.join(temp_filename)
    } else {
        config.output_path.clone()
    };

    let mut writer = csv::WriterBuilder::new().from_path(&target_write_path)?;
    writer.write_record(&headers)?;

    let file_input = File::open(&config.input_path)?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(BufReader::new(file_input));

    let mut row_idx = 2; // header is row 1, data starts at row 2
    let mut cells_updated = 0;
    let mut record = csv::StringRecord::new();

    while reader.read_record(&mut record)? {
        let is_target_row = match config.target.row {
            Some(r) => r == row_idx,
            None => true,
        };

        if is_target_row {
            while record.len() <= col_idx {
                record.push_field("");
            }

            let current_val = &record[col_idx];
            let should_fill = if config.target.row.is_some() {
                true
            } else {
                is_missing_value(current_val.as_bytes())
            };

            if should_fill {
                let mut new_record = csv::StringRecord::new();
                for (i, field) in record.iter().enumerate() {
                    if i == col_idx {
                        new_record.push_field(&imputed_value);
                    } else {
                        new_record.push_field(field);
                    }
                }
                record = new_record;
                cells_updated += 1;
            }
        }

        writer.write_record(&record)?;
        row_idx += 1;
    }

    writer.flush()?;

    if config.in_place {
        std::fs::rename(&target_write_path, &config.input_path)?;
    }

    let final_dest = if config.in_place {
        config.input_path.display().to_string()
    } else {
        config.output_path.display().to_string()
    };

    Ok(FillResult {
        status: "success".to_string(),
        action: "fill".to_string(),
        file_path: config.input_path.display().to_string(),
        output_path: final_dest,
        target_row: config.target.row,
        target_column: col_name,
        target_column_index: col_idx + 1,
        strategy: match &config.strategy {
            FillStrategy::Literal(_) => "literal".to_string(),
            FillStrategy::Mean => "mean".to_string(),
            FillStrategy::Median => "median".to_string(),
            FillStrategy::Mode => "mode".to_string(),
        },
        imputed_value,
        cells_updated,
    })
}

pub fn format_fill_report(result: &FillResult) -> String {
    let mut out = String::new();
    out.push_str("======================================================================\n");
    out.push_str("                     SEARCHUP CSV IMPUTER / FILLER                    \n");
    out.push_str("======================================================================\n");
    out.push_str(&format!("  Input File:      {}\n", result.file_path));
    out.push_str(&format!("  Output File:     {}\n", result.output_path));
    out.push_str(&format!(
        "  Target Column:   {} (Index {})\n",
        result.target_column, result.target_column_index
    ));
    match result.target_row {
        Some(r) => out.push_str(&format!("  Target Row:      Row {}\n", r)),
        None => out.push_str("  Target Scope:    Entire Column (All Missing Values)\n"),
    }
    out.push_str(&format!(
        "  Strategy:        {}\n",
        result.strategy.to_uppercase()
    ));
    out.push_str(&format!("  Imputed Value:   \"{}\"\n", result.imputed_value));
    out.push_str(&format!("  Cells Updated:   {}\n", result.cells_updated));
    out.push_str("======================================================================\n");
    out.push_str("  Status: Successfully completed imputation.\n");
    out.push_str("======================================================================\n");
    out
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
    fn combine(mut self, mut other: Self, limit: Option<usize>) -> Self {
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
        if let Some(lim) = limit {
            self.samples.sort_by_key(|s| (s.row, s.col_idx));
            self.samples.truncate(lim);
        }
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
    let limit = config.limit;
    let headers_arc = std::sync::Arc::new(headers.clone());

    let mut final_stats = thread_pool.install(|| {
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

                                let should_record = match limit {
                                    Some(lim) => stats.samples.len() < lim,
                                    None => true,
                                };

                                if should_record {
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
            .reduce(BatchStats::default, move |a, b| a.combine(b, limit))
    });

    final_stats.samples.sort_by_key(|s| (s.row, s.col_idx));
    if let Some(lim) = limit {
        final_stats.samples.truncate(lim);
    }

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
        if result.sample_occurrences.len() >= result.total_matches {
            out.push_str(&format!(
                "  All Occurrences ({}/{}):\n",
                result.sample_occurrences.len(),
                result.total_matches
            ));
        } else {
            out.push_str(&format!(
                "  Sample Occurrences (showing {} of {}, use --all to view all):\n",
                result.sample_occurrences.len(),
                format_number(result.total_matches)
            ));
        }
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

    #[test]
    fn test_parse_coord() {
        assert_eq!(parse_coord("5,3").unwrap(), (5, "3".to_string()));
        assert_eq!(parse_coord("(5, age)").unwrap(), (5, "age".to_string()));
        assert_eq!(parse_coord("[10, score]").unwrap(), (10, "score".to_string()));
        assert!(parse_coord("invalid").is_err());
        assert!(parse_coord("1,age").is_err()); // row 1 is header
    }
}
