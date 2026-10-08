use searchup::{run_analysis, AnalysisConfig, SearchMode};
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn test_missing_values_detection_accuracy() {
    let mut temp = NamedTempFile::new().unwrap();
    writeln!(temp, "id,name,age,email").unwrap();
    writeln!(temp, "1,Alice,30,alice@example.com").unwrap();
    writeln!(temp, "2,,25,bob@example.com").unwrap(); // missing name
    writeln!(temp, "3,Charlie,NA,").unwrap();          // missing age (NA) and missing email ("")
    writeln!(temp, "4,David,NULL,david@example.com").unwrap(); // missing age (NULL)
    writeln!(temp, "5,Eva,, ").unwrap();              // missing age ("") and email (" ")

    let config = AnalysisConfig::new(temp.path().to_path_buf(), SearchMode::MissingValues, 2);
    let result = run_analysis(&config).expect("Analysis failed");

    assert_eq!(result.total_rows, 5);
    assert_eq!(result.total_columns, 4);
    assert_eq!(result.total_cells, 20);

    // id: 0
    // name: 1 (row 2)
    // age: 3 (row 3 NA, row 4 NULL, row 5 "")
    // email: 2 (row 3 "", row 5 " ")
    // total missing: 6
    assert_eq!(result.total_matches, 6);
    assert_eq!(result.affected_rows, 4); // rows 2, 3, 4, 5

    assert_eq!(result.column_stats[0].match_count, 0); // id
    assert_eq!(result.column_stats[1].match_count, 1); // name
    assert_eq!(result.column_stats[2].match_count, 3); // age
    assert_eq!(result.column_stats[3].match_count, 2); // email
}

#[test]
fn test_worker_consistency() {
    // Generate a CSV with 5,000 rows
    let mut temp = NamedTempFile::new().unwrap();
    writeln!(temp, "id,val1,val2,val3").unwrap();
    for i in 0..5000 {
        let v1 = if i % 3 == 0 { "" } else { "ok" };
        let v2 = if i % 5 == 0 { "NA" } else { "123" };
        let v3 = if i % 10 == 0 { " " } else { "abc" };
        writeln!(temp, "{},{},{},{}", i, v1, v2, v3).unwrap();
    }

    // Run with 1 worker
    let config1 = AnalysisConfig::new(temp.path().to_path_buf(), SearchMode::MissingValues, 1);
    let res1 = run_analysis(&config1).unwrap();

    // Run with 4 workers
    let config4 = AnalysisConfig::new(temp.path().to_path_buf(), SearchMode::MissingValues, 4);
    let res4 = run_analysis(&config4).unwrap();

    // Run with 8 workers and small batch size
    let mut config8 = AnalysisConfig::new(temp.path().to_path_buf(), SearchMode::MissingValues, 8);
    config8.batch_size = 128;
    let res8 = run_analysis(&config8).unwrap();

    assert_eq!(res1.total_rows, 5000);
    assert_eq!(res4.total_rows, 5000);
    assert_eq!(res8.total_rows, 5000);

    assert_eq!(res1.total_matches, res4.total_matches);
    assert_eq!(res1.total_matches, res8.total_matches);

    assert_eq!(res1.affected_rows, res4.affected_rows);
    assert_eq!(res1.affected_rows, res8.affected_rows);

    for (c1, c4) in res1.column_stats.iter().zip(res4.column_stats.iter()) {
        assert_eq!(c1.match_count, c4.match_count);
    }
}

#[test]
fn test_text_search_mode() {
    let mut temp = NamedTempFile::new().unwrap();
    writeln!(temp, "id,city,country").unwrap();
    writeln!(temp, "1,London,United Kingdom").unwrap();
    writeln!(temp, "2,Paris,France").unwrap();
    writeln!(temp, "3,New York,United States").unwrap();
    writeln!(temp, "4,Manchester,United Kingdom").unwrap();

    let config = AnalysisConfig::new(
        temp.path().to_path_buf(),
        SearchMode::Text("Kingdom".to_string()),
        2,
    );
    let result = run_analysis(&config).expect("Analysis failed");

    assert_eq!(result.total_rows, 4);
    assert_eq!(result.total_matches, 2);
    assert_eq!(result.affected_rows, 2);
    assert_eq!(result.column_stats[2].match_count, 2);
}

#[test]
fn test_json_output_structure() {
    let mut temp = NamedTempFile::new().unwrap();
    writeln!(temp, "id,col_a,col_b").unwrap();
    writeln!(temp, "1,,hello").unwrap();
    writeln!(temp, "2,world,null").unwrap();

    let config = AnalysisConfig::new(temp.path().to_path_buf(), SearchMode::MissingValues, 2);
    let result = run_analysis(&config).expect("Analysis failed");
    let json_str = result.to_json(true).expect("JSON conversion failed");

    let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("Valid JSON");
    assert_eq!(parsed["summary"]["total_rows"], 2);
    assert_eq!(parsed["summary"]["total_columns"], 3);
    assert_eq!(parsed["summary"]["total_matches"], 2);
    assert_eq!(parsed["summary"]["affected_rows"], 2);
    assert_eq!(parsed["columns"].as_array().unwrap().len(), 3);
    assert_eq!(parsed["columns"][1]["name"], "col_a");
    assert_eq!(parsed["columns"][1]["match_count"], 1);
    assert_eq!(parsed["columns"][2]["name"], "col_b");
    assert_eq!(parsed["columns"][2]["match_count"], 1);
}

#[test]
fn test_all_occurrences_collection() {
    let mut temp = NamedTempFile::new().unwrap();
    writeln!(temp, "id,col_a,col_b").unwrap();
    // 50 rows, each row has 1 missing value (50 total missing values)
    for i in 1..=50 {
        writeln!(temp, "{},,val", i).unwrap();
    }

    // Default limit (15)
    let config_default = AnalysisConfig::new(temp.path().to_path_buf(), SearchMode::MissingValues, 2);
    let result_default = run_analysis(&config_default).expect("Analysis failed");
    assert_eq!(result_default.total_matches, 50);
    assert_eq!(result_default.sample_occurrences.len(), 15);

    // Unlimited (--all / limit None)
    let config_all = AnalysisConfig::new(temp.path().to_path_buf(), SearchMode::MissingValues, 2)
        .with_limit(None);
    let result_all = run_analysis(&config_all).expect("Analysis failed");
    assert_eq!(result_all.total_matches, 50);
    assert_eq!(result_all.sample_occurrences.len(), 50); // all 50 collected!
}
