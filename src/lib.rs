use colored::Colorize;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum SearchMode {
    MissingValues,
    Outliers,
    Text(String),
}

impl SearchMode {
    pub fn parse(input: &str) -> Self {
        let normalized = input.trim().to_lowercase().replace(['-', ' '], "_");
        if normalized == "missing_values" || normalized == "missing" || normalized == "missing_value" {
            SearchMode::MissingValues
        } else if normalized == "outliers" || normalized == "outlier" || normalized == "anomalies" {
            SearchMode::Outliers
        } else {
            SearchMode::Text(input.trim().to_string())
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            SearchMode::MissingValues => "Missing Values".to_string(),
            SearchMode::Outliers => "Numeric Outliers (IQR)".to_string(),
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
                SearchMode::Outliers => "outliers".to_string(),
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
    let banner_border = "======================================================================".cyan().bold();
    let banner_title = "                     SEARCHUP CSV IMPUTER / FILLER                    ".bright_cyan().bold();
    let divider = "----------------------------------------------------------------------".dimmed();

    out.push_str(&format!("{}\n", banner_border));
    out.push_str(&format!("{}\n", banner_title));
    out.push_str(&format!("{}\n", banner_border));
    out.push_str(&format!("  {:<16} {}\n", "Input File:".bold().white(), result.file_path.cyan()));
    out.push_str(&format!("  {:<16} {}\n", "Output File:".bold().white(), result.output_path.bright_green().bold()));
    out.push_str(&format!(
        "  {:<16} {} {}\n",
        "Target Column:".bold().white(),
        result.target_column.bright_white().bold(),
        format!("(Index {})", result.target_column_index).dimmed()
    ));
    match result.target_row {
        Some(r) => out.push_str(&format!(
            "  {:<16} {}\n",
            "Target Row:".bold().white(),
            format!("Row {}", r).bright_yellow().bold()
        )),
        None => out.push_str(&format!(
            "  {:<16} {}\n",
            "Target Scope:".bold().white(),
            "Entire Column (All Missing Values)".bright_yellow().bold()
        )),
    }
    out.push_str(&format!(
        "  {:<16} {}\n",
        "Strategy:".bold().white(),
        result.strategy.to_uppercase().bright_magenta().bold()
    ));
    out.push_str(&format!(
        "  {:<16} {}\n",
        "Imputed Value:".bold().white(),
        format!("\"{}\"", result.imputed_value).bright_green().bold()
    ));
    out.push_str(&format!(
        "  {:<16} {}\n",
        "Cells Updated:".bold().white(),
        format_number(result.cells_updated).bright_yellow().bold()
    ));
    out.push_str(&format!("{}\n", divider));
    out.push_str(&format!("  {} {}\n", "✓".bright_green().bold(), "Successfully completed imputation.".bright_green()));
    out.push_str(&format!("{}\n", banner_border));
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum InferredType {
    Integer,
    Float,
    Boolean,
    DateTime,
    String,
    Null,
}

impl InferredType {
    pub fn as_str(&self) -> &'static str {
        match self {
            InferredType::Integer => "Integer",
            InferredType::Float => "Float",
            InferredType::Boolean => "Boolean",
            InferredType::DateTime => "DateTime",
            InferredType::String => "String",
            InferredType::Null => "Null",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NumericStats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub std_dev: f64,
    pub median: f64,
    pub q1: f64,
    pub q3: f64,
    pub iqr: f64,
    pub lower_outlier_bound: f64,
    pub upper_outlier_bound: f64,
    pub outlier_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct StringStats {
    pub min_length: usize,
    pub max_length: usize,
    pub avg_length: f64,
    pub top_values: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnProfile {
    pub index: usize,
    pub name: String,
    pub inferred_type: InferredType,
    pub total_rows: usize,
    pub non_null_count: usize,
    pub null_count: usize,
    pub null_percentage: f64,
    pub unique_count: usize,
    pub unique_percentage: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numeric_stats: Option<NumericStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub string_stats: Option<StringStats>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileReport {
    pub file_path: String,
    pub file_size_bytes: u64,
    pub total_rows: usize,
    pub total_columns: usize,
    pub total_cells: usize,
    pub execution_time_ms: f64,
    pub workers_used: usize,
    pub columns: Vec<ColumnProfile>,
}

impl ProfileReport {
    pub fn to_json(&self, pretty: bool) -> Result<String, serde_json::Error> {
        if pretty {
            serde_json::to_string_pretty(self)
        } else {
            serde_json::to_string(self)
        }
    }
}

pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = p * (sorted.len() - 1) as f64;
    let lower_idx = rank.floor() as usize;
    let upper_idx = rank.ceil() as usize;
    if lower_idx == upper_idx {
        sorted[lower_idx]
    } else {
        let weight = rank - lower_idx as f64;
        sorted[lower_idx] * (1.0 - weight) + sorted[upper_idx] * weight
    }
}

pub fn calculate_outlier_bounds(
    file_path: &Path,
    col_count: usize,
) -> Result<HashMap<usize, (f64, f64)>, Box<dyn std::error::Error + Send + Sync>> {
    let file = File::open(file_path)?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(BufReader::with_capacity(128 * 1024, file));

    let mut column_numbers: Vec<Vec<f64>> = vec![Vec::new(); col_count];
    let mut record = csv::ByteRecord::new();

    while reader.read_byte_record(&mut record)? {
        for i in 0..col_count.min(record.len()) {
            let field = &record[i];
            if !is_missing_value(field) {
                if let Ok(s) = std::str::from_utf8(field) {
                    if let Ok(num) = s.trim().parse::<f64>() {
                        column_numbers[i].push(num);
                    }
                }
            }
        }
    }

    let mut bounds = HashMap::new();
    for (i, mut nums) in column_numbers.into_iter().enumerate() {
        if nums.len() >= 4 {
            nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let q1 = percentile(&nums, 0.25);
            let q3 = percentile(&nums, 0.75);
            let iqr = q3 - q1;
            let lower = q1 - 1.5 * iqr;
            let upper = q3 + 1.5 * iqr;
            bounds.insert(i, (lower, upper));
        }
    }

    Ok(bounds)
}

pub fn is_date_or_datetime(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() == 10 && (bytes[4] == b'-' || bytes[4] == b'/') && (bytes[7] == b'-' || bytes[7] == b'/') {
        let year_valid = bytes[0..4].iter().all(|b| b.is_ascii_digit());
        let month_valid = bytes[5..7].iter().all(|b| b.is_ascii_digit());
        let day_valid = bytes[8..10].iter().all(|b| b.is_ascii_digit());
        return year_valid && month_valid && day_valid;
    }
    if bytes.len() >= 19
        && (bytes[4] == b'-' || bytes[4] == b'/')
        && (bytes[7] == b'-' || bytes[7] == b'/')
        && (bytes[10] == b' ' || bytes[10] == b'T')
        && bytes[13] == b':'
        && bytes[16] == b':'
    {
        let y_ok = bytes[0..4].iter().all(|b| b.is_ascii_digit());
        let m_ok = bytes[5..7].iter().all(|b| b.is_ascii_digit());
        let d_ok = bytes[8..10].iter().all(|b| b.is_ascii_digit());
        let h_ok = bytes[11..13].iter().all(|b| b.is_ascii_digit());
        let min_ok = bytes[14..16].iter().all(|b| b.is_ascii_digit());
        let sec_ok = bytes[17..19].iter().all(|b| b.is_ascii_digit());
        return y_ok && m_ok && d_ok && h_ok && min_ok && sec_ok;
    }
    false
}

pub fn detect_value_type(val: &str) -> InferredType {
    let trimmed = val.trim();
    if is_missing_value(trimmed.as_bytes()) {
        return InferredType::Null;
    }
    if trimmed.eq_ignore_ascii_case("true")
        || trimmed.eq_ignore_ascii_case("false")
        || trimmed.eq_ignore_ascii_case("yes")
        || trimmed.eq_ignore_ascii_case("no")
    {
        return InferredType::Boolean;
    }
    if is_date_or_datetime(trimmed) {
        return InferredType::DateTime;
    }
    if trimmed.parse::<i64>().is_ok() {
        return InferredType::Integer;
    }
    if trimmed.parse::<f64>().is_ok() {
        return InferredType::Float;
    }
    InferredType::String
}

#[derive(Clone)]
struct ColumnAccumulator {
    non_null_count: usize,
    null_count: usize,
    int_count: usize,
    float_count: usize,
    bool_count: usize,
    date_count: usize,
    string_count: usize,
    min_len: usize,
    max_len: usize,
    sum_len: usize,
    unique_set: HashSet<String>,
    numbers: Vec<f64>,
    freq: HashMap<String, usize>,
}

impl ColumnAccumulator {
    fn new() -> Self {
        Self {
            non_null_count: 0,
            null_count: 0,
            int_count: 0,
            float_count: 0,
            bool_count: 0,
            date_count: 0,
            string_count: 0,
            min_len: usize::MAX,
            max_len: 0,
            sum_len: 0,
            unique_set: HashSet::new(),
            numbers: Vec::new(),
            freq: HashMap::new(),
        }
    }

    fn combine(&mut self, other: Self) {
        self.non_null_count += other.non_null_count;
        self.null_count += other.null_count;
        self.int_count += other.int_count;
        self.float_count += other.float_count;
        self.bool_count += other.bool_count;
        self.date_count += other.date_count;
        self.string_count += other.string_count;
        self.min_len = self.min_len.min(other.min_len);
        self.max_len = self.max_len.max(other.max_len);
        self.sum_len += other.sum_len;

        if self.unique_set.len() < 50_000 {
            for val in other.unique_set {
                if self.unique_set.len() >= 50_000 {
                    break;
                }
                self.unique_set.insert(val);
            }
        }

        self.numbers.extend(other.numbers);

        for (k, v) in other.freq {
            if self.freq.len() < 1_000 || self.freq.contains_key(&k) {
                *self.freq.entry(k).or_insert(0) += v;
            }
        }
    }
}

struct BatchProfileResult {
    row_count: usize,
    accumulators: Vec<ColumnAccumulator>,
    error: Option<String>,
}

impl BatchProfileResult {
    fn combine(mut self, mut other: Self) -> Self {
        if self.error.is_none() {
            self.error = other.error.take();
        }
        self.row_count += other.row_count;
        if other.accumulators.len() > self.accumulators.len() {
            self.accumulators.resize_with(other.accumulators.len(), ColumnAccumulator::new);
        }
        for (i, acc) in other.accumulators.into_iter().enumerate() {
            if i < self.accumulators.len() {
                self.accumulators[i].combine(acc);
            } else {
                self.accumulators.push(acc);
            }
        }
        self
    }
}

pub fn run_profile(
    config: &AnalysisConfig,
) -> Result<ProfileReport, Box<dyn std::error::Error + Send + Sync>> {
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
        let mut row_idx = 2;
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

    let final_profile = thread_pool.install(|| {
        rx.into_iter()
            .par_bridge()
            .map(|batch_res| match batch_res {
                Ok(batch) => {
                    let mut accumulators = (0..col_count)
                        .map(|_| ColumnAccumulator::new())
                        .collect::<Vec<_>>();
                    let row_count = batch.len();

                    for (_row_idx, record) in batch {
                        for i in 0..col_count {
                            if i < record.len() {
                                let field = &record[i];
                                if is_missing_value(field) {
                                    accumulators[i].null_count += 1;
                                } else {
                                    accumulators[i].non_null_count += 1;
                                    let s = String::from_utf8_lossy(field).trim().to_string();
                                    let len = s.len();
                                    if len < accumulators[i].min_len {
                                        accumulators[i].min_len = len;
                                    }
                                    if len > accumulators[i].max_len {
                                        accumulators[i].max_len = len;
                                    }
                                    accumulators[i].sum_len += len;

                                    if accumulators[i].unique_set.len() < 50_000 {
                                        accumulators[i].unique_set.insert(s.clone());
                                    }
                                    *accumulators[i].freq.entry(s.clone()).or_insert(0) += 1;

                                    match detect_value_type(&s) {
                                        InferredType::Integer => {
                                            accumulators[i].int_count += 1;
                                            if let Ok(n) = s.parse::<f64>() {
                                                accumulators[i].numbers.push(n);
                                            }
                                        }
                                        InferredType::Float => {
                                            accumulators[i].float_count += 1;
                                            if let Ok(n) = s.parse::<f64>() {
                                                accumulators[i].numbers.push(n);
                                            }
                                        }
                                        InferredType::Boolean => {
                                            accumulators[i].bool_count += 1;
                                        }
                                        InferredType::DateTime => {
                                            accumulators[i].date_count += 1;
                                        }
                                        _ => {
                                            accumulators[i].string_count += 1;
                                        }
                                    }
                                }
                            } else {
                                accumulators[i].null_count += 1;
                            }
                        }
                    }

                    BatchProfileResult {
                        row_count,
                        accumulators,
                        error: None,
                    }
                }
                Err(err) => BatchProfileResult {
                    row_count: 0,
                    accumulators: (0..col_count).map(|_| ColumnAccumulator::new()).collect(),
                    error: Some(err),
                },
            })
            .reduce(
                || BatchProfileResult {
                    row_count: 0,
                    accumulators: (0..col_count).map(|_| ColumnAccumulator::new()).collect(),
                    error: None,
                },
                BatchProfileResult::combine,
            )
    });

    if let Err(e) = producer.join() {
        return Err(format!("Producer thread panicked: {:?}", e).into());
    }

    if let Some(err_msg) = final_profile.error {
        return Err(err_msg.into());
    }

    let total_rows = final_profile.row_count;
    let total_cells = total_rows * col_count;

    let columns: Vec<ColumnProfile> = (0..col_count)
        .into_par_iter()
        .map(|i| {
            let acc = &final_profile.accumulators[i];
            let name = headers[i].clone();

            let inferred_type = if acc.non_null_count == 0 {
                InferredType::Null
            } else if acc.bool_count == acc.non_null_count {
                InferredType::Boolean
            } else if acc.date_count == acc.non_null_count {
                InferredType::DateTime
            } else if acc.int_count == acc.non_null_count {
                InferredType::Integer
            } else if (acc.int_count + acc.float_count) == acc.non_null_count {
                InferredType::Float
            } else {
                InferredType::String
            };

            let null_pct = if total_rows > 0 {
                (acc.null_count as f64 / total_rows as f64) * 100.0
            } else {
                0.0
            };
            let unique_pct = if total_rows > 0 {
                (acc.unique_set.len() as f64 / total_rows as f64) * 100.0
            } else {
                0.0
            };

            let numeric_stats = if (inferred_type == InferredType::Integer || inferred_type == InferredType::Float)
                && !acc.numbers.is_empty()
            {
                let mut sorted = acc.numbers.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let min = sorted[0];
                let max = sorted[sorted.len() - 1];
                let sum: f64 = sorted.iter().sum();
                let mean = sum / sorted.len() as f64;
                let var: f64 =
                    sorted.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / sorted.len() as f64;
                let std_dev = var.sqrt();
                let median = percentile(&sorted, 0.50);
                let q1 = percentile(&sorted, 0.25);
                let q3 = percentile(&sorted, 0.75);
                let iqr = q3 - q1;
                let lower_bound = q1 - 1.5 * iqr;
                let upper_bound = q3 + 1.5 * iqr;
                let outlier_count = sorted
                    .iter()
                    .filter(|&&x| x < lower_bound || x > upper_bound)
                    .count();

                Some(NumericStats {
                    min: (min * 100.0).round() / 100.0,
                    max: (max * 100.0).round() / 100.0,
                    mean: (mean * 100.0).round() / 100.0,
                    std_dev: (std_dev * 100.0).round() / 100.0,
                    median: (median * 100.0).round() / 100.0,
                    q1: (q1 * 100.0).round() / 100.0,
                    q3: (q3 * 100.0).round() / 100.0,
                    iqr: (iqr * 100.0).round() / 100.0,
                    lower_outlier_bound: (lower_bound * 100.0).round() / 100.0,
                    upper_outlier_bound: (upper_bound * 100.0).round() / 100.0,
                    outlier_count,
                })
            } else {
                None
            };

            let string_stats = if inferred_type == InferredType::String
                || inferred_type == InferredType::Boolean
                || inferred_type == InferredType::DateTime
            {
                let avg_len = if acc.non_null_count > 0 {
                    acc.sum_len as f64 / acc.non_null_count as f64
                } else {
                    0.0
                };
                let mut freq_vec: Vec<(String, usize)> = acc.freq.clone().into_iter().collect();
                freq_vec.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                freq_vec.truncate(3);

                Some(StringStats {
                    min_length: if acc.min_len == usize::MAX {
                        0
                    } else {
                        acc.min_len
                    },
                    max_length: acc.max_len,
                    avg_length: (avg_len * 10.0).round() / 10.0,
                    top_values: freq_vec,
                })
            } else {
                None
            };

            ColumnProfile {
                index: i + 1,
                name,
                inferred_type,
                total_rows,
                non_null_count: acc.non_null_count,
                null_count: acc.null_count,
                null_percentage: (null_pct * 100.0).round() / 100.0,
                unique_count: acc.unique_set.len(),
                unique_percentage: (unique_pct * 100.0).round() / 100.0,
                numeric_stats,
                string_stats,
            }
        })
        .collect();

    let duration = start_time.elapsed();

    Ok(ProfileReport {
        file_path: config.file_path.display().to_string(),
        file_size_bytes,
        total_rows,
        total_columns: col_count,
        total_cells,
        execution_time_ms: (duration.as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
        workers_used: config.workers,
        columns,
    })
}

pub fn format_profile_report(report: &ProfileReport) -> String {
    let mut out = String::new();
    let border = "========================================================================================================".cyan().bold();
    let title = "                                      SEARCHUP DATASET PROFILER                                         ".bright_cyan().bold();
    let divider = "--------------------------------------------------------------------------------------------------------".dimmed();

    out.push_str(&format!("{}\n", border));
    out.push_str(&format!("{}\n", title));
    out.push_str(&format!("{}\n", border));
    out.push_str(&format!("  {:<16} {}\n", "File:".bold().white(), report.file_path.cyan()));
    out.push_str(&format!("  {:<16} {}\n", "File Size:".bold().white(), format_size(report.file_size_bytes).bright_yellow()));
    out.push_str(&format!("  {:<16} {}\n", "Data Rows:".bold().white(), format_number(report.total_rows).bright_yellow()));
    out.push_str(&format!("  {:<16} {}\n", "Columns:".bold().white(), report.total_columns.to_string().bright_yellow()));
    out.push_str(&format!("  {:<16} {}\n", "Total Cells:".bold().white(), format_number(report.total_cells).bright_yellow()));
    out.push_str(&format!("  {:<16} {}\n", "Workers:".bold().white(), report.workers_used.to_string().bright_green()));
    out.push_str(&format!("{}\n", divider));

    let header_line = format!(
        "  {:<3} {:<16} {:<10} {:>9} {:>9} {:>8}  {}",
        "#", "Column Name", "Type", "Nulls", "Null %", "Uniques", "Stats / Summary"
    );
    out.push_str(&format!("{}\n", header_line.bold()));
    out.push_str(&format!("{}\n", divider));

    for col in &report.columns {
        let stats_summary = if let Some(num) = &col.numeric_stats {
            let outliers_fmt = if num.outlier_count > 0 {
                format!("outliers: {}", num.outlier_count).bright_red().bold().to_string()
            } else {
                "outliers: 0".dimmed().to_string()
            };
            format!(
                "min: {}, max: {}, mean: {:.2}, med: {:.2}, {}",
                num.min, num.max, num.mean, num.median, outliers_fmt
            )
        } else if let Some(str_s) = &col.string_stats {
            let top_str = str_s
                .top_values
                .iter()
                .map(|(v, c)| format!("\"{}\" ({})", v.bright_cyan(), c.to_string().dimmed()))
                .collect::<Vec<_>>()
                .join(", ");
            if top_str.is_empty() {
                format!("len: [{}..{}]", str_s.min_length, str_s.max_length)
            } else {
                format!("len: [{}..{}], top: {}", str_s.min_length, str_s.max_length, top_str)
            }
        } else {
            "-".dimmed().to_string()
        };

        let col_name_display = if col.name.len() > 16 {
            format!("{}...", &col.name[..13])
        } else {
            col.name.clone()
        };

        let idx_str = format!("{:<3}", col.index).dimmed();
        let name_str = format!("{:<16}", col_name_display).bright_white().bold();

        let type_raw = format!("{:<10}", col.inferred_type.as_str());
        let type_str = match col.inferred_type {
            InferredType::Integer => type_raw.bright_cyan().bold(),
            InferredType::Float => type_raw.bright_blue().bold(),
            InferredType::Boolean => type_raw.bright_magenta().bold(),
            InferredType::DateTime => type_raw.bright_yellow().bold(),
            InferredType::String => type_raw.bright_green().bold(),
            InferredType::Null => type_raw.dimmed(),
        };

        let null_count_raw = format!("{:>9}", format_number(col.null_count));
        let null_pct_raw = format!("{:>8.1}%", col.null_percentage);

        let (null_count_str, null_pct_str) = if col.null_count == 0 {
            (null_count_raw.dimmed(), null_pct_raw.dimmed())
        } else if col.null_percentage > 15.0 {
            (null_count_raw.bright_red().bold(), null_pct_raw.bright_red().bold())
        } else {
            (null_count_raw.bright_yellow(), null_pct_raw.bright_yellow())
        };

        let unique_str = format!("{:>8}", format_number(col.unique_count)).white();

        out.push_str(&format!(
            "  {} {} {} {} {} {}  {}\n",
            idx_str,
            name_str,
            type_str,
            null_count_str,
            null_pct_str,
            unique_str,
            stats_summary
        ));
    }

    out.push_str(&format!("{}\n", divider));
    let rows_sec = if report.execution_time_ms > 0.0 {
        (report.total_rows as f64 / (report.execution_time_ms / 1000.0)).round()
    } else {
        0.0
    };
    out.push_str(&format!(
        "  {:<18} {}\n",
        "Execution Time:".bold().white(),
        format!("{:.2} ms ({:.0} rows/sec)", report.execution_time_ms, rows_sec).bright_green().bold()
    ));
    out.push_str(&format!("{}\n", border));
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
        SearchMode::Outliers => false,
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

    let outlier_bounds = if matches!(config.mode, SearchMode::Outliers) {
        calculate_outlier_bounds(&config.file_path, col_count)?
    } else {
        HashMap::new()
    };
    let outlier_bounds_arc = std::sync::Arc::new(outlier_bounds);

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
                                match &mode {
                                    SearchMode::MissingValues => {
                                        if is_missing_value(field) {
                                            let s = String::from_utf8_lossy(field).trim().to_string();
                                            let prev = if s.is_empty() { "<empty>".to_string() } else { s };
                                            (true, prev)
                                        } else {
                                            (false, String::new())
                                        }
                                    }
                                    SearchMode::Outliers => {
                                        if let Some(&(lower, upper)) = outlier_bounds_arc.get(&i) {
                                            if !is_missing_value(field) {
                                                if let Ok(s) = std::str::from_utf8(field) {
                                                    if let Ok(val) = s.trim().parse::<f64>() {
                                                        if val < lower || val > upper {
                                                            (true, format!("{val} (expected [{lower:.2}, {upper:.2}])"))
                                                        } else {
                                                            (false, String::new())
                                                        }
                                                    } else {
                                                        (false, String::new())
                                                    }
                                                } else {
                                                    (false, String::new())
                                                }
                                            } else {
                                                (false, String::new())
                                            }
                                        } else {
                                            (false, String::new())
                                        }
                                    }
                                    SearchMode::Text(_) => {
                                        if field_matches(field, &mode) {
                                            let s = String::from_utf8_lossy(field).trim().to_string();
                                            (true, s)
                                        } else {
                                            (false, String::new())
                                        }
                                    }
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
    let target_label = match &result.search_mode {
        SearchMode::MissingValues => "Missing Values",
        SearchMode::Outliers => "Outliers",
        SearchMode::Text(_) => "Matches",
    };

    let border = "======================================================================".cyan().bold();
    let banner_title = "                        SEARCHUP CSV ANALYZER                         ".bright_cyan().bold();
    let divider = "----------------------------------------------------------------------".dimmed();

    out.push_str(&format!("{}\n", border));
    out.push_str(&format!("{}\n", banner_title));
    out.push_str(&format!("{}\n", border));
    out.push_str(&format!("  {:<18} {}\n", "File:".bold().white(), result.file_path.display().to_string().cyan()));
    out.push_str(&format!("  {:<18} {}\n", "File Size:".bold().white(), format_size(result.file_size_bytes).bright_yellow()));
    out.push_str(&format!("  {:<18} {}\n", "Search Target:".bold().white(), result.search_mode.display_name().bright_magenta().bold()));
    out.push_str(&format!("  {:<18} {}\n", "Parallel Workers:".bold().white(), result.workers_used.to_string().bright_green()));
    out.push_str(&format!("  {:<18} {}\n", "Data Rows:".bold().white(), format_number(result.total_rows).bright_yellow()));
    out.push_str(&format!("  {:<18} {}\n", "Columns:".bold().white(), result.total_columns.to_string().bright_yellow()));
    out.push_str(&format!("  {:<18} {}\n", "Total Cells:".bold().white(), format_number(result.total_cells).bright_yellow()));
    out.push_str(&format!("{}\n", divider));

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

    let total_matches_str = format_number(result.total_matches);
    let total_matches_colored = if result.total_matches == 0 {
        total_matches_str.bright_green().bold()
    } else if cell_pct > 10.0 {
        total_matches_str.bright_red().bold()
    } else {
        total_matches_str.bright_yellow().bold()
    };

    let cell_pct_colored = if cell_pct == 0.0 {
        format!("{:.2}%", cell_pct).bright_green()
    } else if cell_pct > 10.0 {
        format!("{:.2}%", cell_pct).bright_red().bold()
    } else {
        format!("{:.2}%", cell_pct).bright_yellow()
    };

    out.push_str(&format!(
        "  {:<22} {} ({} of all cells)\n",
        format!("Total {}:", target_label).bold().white(),
        total_matches_colored,
        cell_pct_colored
    ));

    let affected_rows_str = format_number(result.affected_rows);
    let affected_rows_colored = if result.affected_rows == 0 {
        affected_rows_str.bright_green().bold()
    } else {
        affected_rows_str.bright_yellow().bold()
    };
    let row_pct_colored = if row_pct == 0.0 {
        format!("{:.2}%", row_pct).bright_green()
    } else {
        format!("{:.2}%", row_pct).bright_yellow()
    };

    out.push_str(&format!(
        "  {:<22} {} ({} of data rows)\n",
        "Rows Affected:".bold().white(),
        affected_rows_colored,
        row_pct_colored
    ));
    out.push_str(&format!("{}\n", divider));
    out.push_str(&format!("  {}:\n", format!("Breakdown by Column ({})", target_label).bold().bright_white()));

    // Find max column name length for clean alignment
    let max_name_len = result
        .column_stats
        .iter()
        .map(|c| c.name.len())
        .max()
        .unwrap_or(12)
        .max(14)
        .min(30);

    let target_header_width = target_label.len().max(14);
    let col_header = format!(
        "    {:<4} {:<width$} {:>t_width$} {:>10}  {}",
        "#",
        "Column Name",
        target_label,
        "Percentage",
        "Distribution",
        width = max_name_len,
        t_width = target_header_width
    );
    out.push_str(&format!("{}\n", col_header.bold()));

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

        let idx_str = format!("{:<4}", col.index).dimmed();
        let name_str = format!("{:<width$}", display_name, width = max_name_len).bright_white().bold();

        let count_num = format_number(col.match_count);
        let count_str = if col.match_count == 0 {
            format!("{:>t_width$}", count_num, t_width = target_header_width).dimmed()
        } else if col_pct > 15.0 {
            format!("{:>t_width$}", count_num, t_width = target_header_width).bright_red().bold()
        } else {
            format!("{:>t_width$}", count_num, t_width = target_header_width).bright_yellow()
        };

        let pct_num = format!("{:>9.2}%", col_pct);
        let pct_str = if col.match_count == 0 {
            pct_num.dimmed()
        } else if col_pct > 15.0 {
            pct_num.bright_red().bold()
        } else {
            pct_num.bright_yellow()
        };

        let bar_display = format!("{}{}{}", "[".dimmed(), bar, "]".dimmed());

        out.push_str(&format!(
            "    {} {} {} {}  {}\n",
            idx_str,
            name_str,
            count_str,
            pct_str,
            bar_display
        ));
    }

    if !result.sample_occurrences.is_empty() {
        out.push_str(&format!("{}\n", divider));
        if result.sample_occurrences.len() >= result.total_matches {
            out.push_str(&format!(
                "  {}\n",
                format!("All Occurrences ({}/{}):", result.sample_occurrences.len(), result.total_matches).bold().bright_yellow()
            ));
        } else {
            out.push_str(&format!(
                "  {} {}\n",
                format!("Sample Occurrences (showing {} of {},", result.sample_occurrences.len(), format_number(result.total_matches)).bold().bright_yellow(),
                "use --all to view all):".dimmed()
            ));
        }
        for occ in &result.sample_occurrences {
            let val_display = if occ.value_preview.is_empty()
                || occ.value_preview.eq_ignore_ascii_case("<empty>")
            {
                "<empty>".bright_red().bold().to_string()
            } else if occ.value_preview.eq_ignore_ascii_case("null")
                || occ.value_preview.eq_ignore_ascii_case("na")
                || occ.value_preview.eq_ignore_ascii_case("n/a")
                || occ.value_preview.eq_ignore_ascii_case("nan")
                || occ.value_preview.eq_ignore_ascii_case("none")
                || occ.value_preview == "?"
            {
                occ.value_preview.bright_red().bold().to_string()
            } else {
                occ.value_preview.bright_yellow().to_string()
            };

            out.push_str(&format!(
                "    {} Row {:<6} {} Column {:<2} ({}) -> {}\n",
                "-".dimmed(),
                format!("{}", occ.row).cyan(),
                "|".dimmed(),
                format!("{}", occ.col_idx + 1).cyan(),
                format!("\"{}\"", occ.col_name).bright_white().bold(),
                val_display
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

    out.push_str(&format!("{}\n", border));
    out.push_str(&format!(
        "  {:<18} {}\n",
        "Execution Time:".bold().white(),
        format!("{:.2?} ({:.1} rows/sec, {:.1} cells/sec)", result.duration, rows_per_sec, cells_per_sec).bright_green().bold()
    ));
    out.push_str(&format!("{}\n", border));

    out
}

fn generate_progress_bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width.saturating_sub(filled);

    if filled == 0 {
        " ".repeat(width)
    } else {
        let bar_chars = "=".repeat(filled);
        let colored_bar = if pct > 20.0 {
            bar_chars.bright_red().bold()
        } else if pct > 5.0 {
            bar_chars.bright_yellow().bold()
        } else {
            bar_chars.bright_green().bold()
        };
        format!("{}{}", colored_bar, " ".repeat(empty))
    }
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

    #[test]
    fn test_report_formatting() {
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

        let report = format_report(&result);
        assert!(report.contains("SEARCHUP CSV ANALYZER"));
        assert!(report.contains("col1"));
        assert!(report.contains("col2"));

        let fill_res = FillResult {
            status: "success".to_string(),
            action: "fill".to_string(),
            file_path: "input.csv".to_string(),
            output_path: "output.csv".to_string(),
            target_row: Some(5),
            target_column: "age".to_string(),
            target_column_index: 2,
            strategy: "mean".to_string(),
            imputed_value: "35.5".to_string(),
            cells_updated: 1,
        };
        let fill_report = format_fill_report(&fill_res);
        assert!(fill_report.contains("SEARCHUP CSV IMPUTER / FILLER"));
        assert!(fill_report.contains("Row 5"));
        assert!(fill_report.contains("35.5"));

        let prof_report = ProfileReport {
            file_path: "input.csv".to_string(),
            file_size_bytes: 512,
            workers_used: 2,
            total_rows: 100,
            total_columns: 1,
            total_cells: 100,
            execution_time_ms: 10.0,
            columns: vec![ColumnProfile {
                index: 1,
                name: "age".to_string(),
                inferred_type: InferredType::Integer,
                total_rows: 100,
                non_null_count: 95,
                null_count: 5,
                null_percentage: 5.0,
                unique_count: 50,
                unique_percentage: 50.0,
                numeric_stats: None,
                string_stats: None,
            }],
        };
        let prof_formatted = format_profile_report(&prof_report);
        assert!(prof_formatted.contains("SEARCHUP DATASET PROFILER"));
        assert!(prof_formatted.contains("Integer"));
    }
}
