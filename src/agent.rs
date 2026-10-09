use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{
    execute_fill, run_analysis, run_profile, AnalysisConfig, FillConfig, FillStrategy, FillTarget,
    SearchMode,
};

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub file_path: PathBuf,
    pub api_key: Option<String>,
    pub api_base: String,
    pub model: String,
    pub max_turns: usize,
    pub workers: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self {
            file_path: PathBuf::new(),
            api_key: None,
            api_base: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o-mini".to_string(),
            max_turns: 12,
            workers,
        }
    }
}

// === OpenAI-Compatible API Structures ===

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
    pub choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
}

// === Agent Tools Specification ===

pub fn get_agent_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "profile_dataset".to_string(),
                description: "Inspect the dataset schema, inferred column types (Integer, Float, Boolean, DateTime, String), null counts, unique counts, distributions, and outlier boundaries.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "search_missing_values".to_string(),
                description: "Scan for missing, null, empty, NA, or placeholder values across all columns with exact row and column occurrences.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of occurrences to return (default: 20)"
                        }
                    },
                    "required": []
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "search_outliers".to_string(),
                description: "Detect numeric outliers across all numeric columns using Tukey's robust Interquartile Range (IQR) rule.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "description": "Maximum outlier occurrences to return (default: 20)"
                        }
                    },
                    "required": []
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "fill_missing_value".to_string(),
                description: "Impute or fill missing values in a column or specific coordinate using strategies ('mean', 'median', 'mode', or a literal value).".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "column": {
                            "type": "string",
                            "description": "Target column name or 1-based index (e.g. 'age' or '3')"
                        },
                        "row": {
                            "type": "integer",
                            "description": "Optional target row number (1-based line number). If omitted, fills all missing values across the column."
                        },
                        "value": {
                            "type": "string",
                            "description": "Imputation strategy ('mean', 'median', 'mode') or a literal replacement value (e.g. '0', 'Unknown')."
                        },
                        "in_place": {
                            "type": "boolean",
                            "description": "Whether to safely overwrite the loaded file in-place (default: true)."
                        }
                    },
                    "required": ["column", "value"]
                }),
            },
        },
    ]
}

// === Tool Execution Engine ===

pub fn execute_tool(
    name: &str,
    args_json: &str,
    config: &AgentConfig,
) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(args_json)
        .map_err(|e| format!("Invalid JSON arguments: {}", e))?;

    match name {
        "profile_dataset" => {
            let a_config = AnalysisConfig::new(
                config.file_path.clone(),
                SearchMode::MissingValues,
                config.workers,
            );
            let profile_report = run_profile(&a_config)
                .map_err(|e| format!("Profiling error: {}", e))?;
            profile_report
                .to_json(false)
                .map_err(|e| format!("JSON serialization error: {}", e))
        }

        "search_missing_values" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).map(|n| n as usize).or(Some(20));
            let mut a_config = AnalysisConfig::new(
                config.file_path.clone(),
                SearchMode::MissingValues,
                config.workers,
            );
            a_config.limit = limit;

            let result = run_analysis(&a_config)
                .map_err(|e| format!("Analysis error: {}", e))?;
            result
                .to_json(false)
                .map_err(|e| format!("JSON serialization error: {}", e))
        }

        "search_outliers" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).map(|n| n as usize).or(Some(20));
            let mut a_config = AnalysisConfig::new(
                config.file_path.clone(),
                SearchMode::Outliers,
                config.workers,
            );
            a_config.limit = limit;

            let result = run_analysis(&a_config)
                .map_err(|e| format!("Outlier analysis error: {}", e))?;
            result
                .to_json(false)
                .map_err(|e| format!("JSON serialization error: {}", e))
        }

        "fill_missing_value" => {
            let column = args
                .get("column")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter 'column'".to_string())?
                .to_string();

            let val_str = args
                .get("value")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing required parameter 'value'".to_string())?;

            let row = args.get("row").and_then(|v| v.as_u64()).map(|n| n as usize);
            let in_place = args.get("in_place").and_then(|v| v.as_bool()).unwrap_or(true);

            let strategy = FillStrategy::parse(val_str);
            let output_path = if in_place {
                config.file_path.clone()
            } else {
                let stem = config
                    .file_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("output");
                let ext = config
                    .file_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("csv");
                let parent = config
                    .file_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."));
                parent.join(format!("{}_filled.{}", stem, ext))
            };

            let fill_cfg = FillConfig {
                input_path: config.file_path.clone(),
                output_path,
                in_place,
                target: FillTarget {
                    row,
                    col: column,
                },
                strategy,
            };

            let result = execute_fill(&fill_cfg)
                .map_err(|e| format!("Fill execution error: {}", e))?;
            result
                .to_json(false)
                .map_err(|e| format!("JSON serialization error: {}", e))
        }

        unknown => Err(format!("Unknown tool: {}", unknown)),
    }
}

// === Tool Call Fallback Parser for Local Models ===

pub fn extract_fallback_tool_calls(
    content: &str,
    valid_tools: &[ToolDefinition],
) -> Option<Vec<ToolCall>> {
    let valid_names: std::collections::HashSet<&str> =
        valid_tools.iter().map(|t| t.function.name.as_str()).collect();

    let trimmed = content.trim();

    // Strip markdown code fences if present (e.g. ```json ... ```)
    let json_str = if trimmed.starts_with("```") {
        let lines: Vec<&str> = trimmed.lines().collect();
        if lines.len() >= 2 {
            let start = 1;
            let end = if lines.last().map_or(false, |l| l.trim().starts_with("```")) {
                lines.len() - 1
            } else {
                lines.len()
            };
            lines[start..end].join("\n")
        } else {
            trimmed.to_string()
        }
    } else {
        trimmed.to_string()
    };

    // 1. Try parsing entire string as JSON Value directly
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json_str) {
        if let Some(arr) = val.as_array() {
            let mut calls = Vec::new();
            for item in arr {
                if let Some(call) = parse_single_tool_call(item, &valid_names) {
                    calls.push(call);
                }
            }
            if !calls.is_empty() {
                return Some(calls);
            }
        } else if let Some(call) = parse_single_tool_call(&val, &valid_names) {
            return Some(vec![call]);
        }
    }

    // 2. Try handling malformed double/triple braces (e.g. {{...}}})
    let sanitized = json_str.replace("{{{", "{").replace("{{", "{").replace("}}}", "}}");
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&sanitized) {
        if let Some(call) = parse_single_tool_call(&val, &valid_names) {
            return Some(vec![call]);
        }
    }

    // 3. Search for JSON object substring: {...}
    if let Some(start_idx) = json_str.find('{') {
        if let Some(end_idx) = json_str.rfind('}') {
            if end_idx > start_idx {
                let candidate = &json_str[start_idx..=end_idx];
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(candidate) {
                    if let Some(call) = parse_single_tool_call(&val, &valid_names) {
                        return Some(vec![call]);
                    }
                }
                // Try candidate with sanitized braces
                let cand_sanitized = candidate.replace("{{{", "{").replace("{{", "{").replace("}}}", "}}");
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cand_sanitized) {
                    if let Some(call) = parse_single_tool_call(&val, &valid_names) {
                        return Some(vec![call]);
                    }
                }
            }
        }
    }

    None
}

fn parse_single_tool_call(
    val: &serde_json::Value,
    valid_names: &std::collections::HashSet<&str>,
) -> Option<ToolCall> {
    let obj = val.as_object()?;
    let name = obj
        .get("name")
        .or_else(|| obj.get("function"))
        .and_then(|v| v.as_str())?;

    if !valid_names.contains(name) {
        return None;
    }

    let args_val = obj
        .get("arguments")
        .or_else(|| obj.get("parameters"))
        .cloned()
        .unwrap_or(serde_json::json!({}));

    let arguments = if let Some(s) = args_val.as_str() {
        s.to_string()
    } else {
        args_val.to_string()
    };

    Some(ToolCall {
        id: format!("call_fallback_{}", name),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: name.to_string(),
            arguments,
        },
    })
}

// === Agentic Loop Engine ===

pub async fn run_agent_turn(
    client: &reqwest::Client,
    config: &AgentConfig,
    messages: &mut Vec<ChatMessage>,
    tools: &[ToolDefinition],
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/chat/completions", config.api_base.trim_end_matches('/'));

    let mut turn_count = 0;
    while turn_count < config.max_turns {
        turn_count += 1;

        let request = ChatCompletionRequest {
            model: config.model.clone(),
            messages: messages.clone(),
            tools: tools.to_vec(),
        };

        let mut req_builder = client
            .post(&url)
            .header("Content-Type", "application/json")
            .timeout(Duration::from_secs(300))
            .json(&request);

        if let Some(key) = &config.api_key {
            if !key.is_empty() {
                req_builder = req_builder.header("Authorization", format!("Bearer {}", key));
            }
        }

        print!("  {} {}\r", "⏳".dimmed(), "Thinking & querying model...".dimmed());
        let _ = io::stdout().flush();

        let resp = req_builder.send().await;
        // Clear progress line
        print!("\r{}\r", " ".repeat(40));
        let _ = io::stdout().flush();

        let resp = resp?;
        let status = resp.status();
        if !status.is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "API request failed with status {}: {}",
                status, err_text
            )
            .into());
        }

        let completion: ChatCompletionResponse = resp.json().await?;
        let choice = completion
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| "No completion choice returned by model")?;

        let mut assistant_message = choice.message;

        // Fallback for smaller local models (e.g. Qwen, LLaMA) that output JSON tool calls in `content`
        if assistant_message.tool_calls.as_ref().map_or(true, |tc| tc.is_empty()) {
            if let Some(content) = &assistant_message.content {
                if let Some(extracted) = extract_fallback_tool_calls(content, tools) {
                    assistant_message.tool_calls = Some(extracted);
                    assistant_message.content = None;
                }
            }
        }

        // If the model produced tool calls, execute each tool call
        if let Some(tool_calls) = &assistant_message.tool_calls {
            if !tool_calls.is_empty() {
                // Record assistant message with tool calls in history
                messages.push(assistant_message.clone());

                for call in tool_calls {
                    let fn_name = &call.function.name;
                    let fn_args = &call.function.arguments;

                    println!(
                        "  {} {} {}",
                        "⚙".bright_cyan().bold(),
                        "Invoking Tool:".bold().white(),
                        format!("{}({})", fn_name.bright_magenta().bold(), fn_args.dimmed())
                    );

                    let tool_result = match execute_tool(fn_name, fn_args, config) {
                        Ok(res) => {
                            println!(
                                "  {} {}\n",
                                "✓".bright_green().bold(),
                                "Tool executed successfully.".green()
                            );
                            res
                        }
                        Err(err) => {
                            println!(
                                "  {} {}: {}\n",
                                "✗".bright_red().bold(),
                                "Tool execution error".red(),
                                err
                            );
                            serde_json::json!({
                                "status": "error",
                                "error": err
                            })
                            .to_string()
                        }
                    };

                    messages.push(ChatMessage {
                        role: "tool".to_string(),
                        content: Some(tool_result),
                        tool_calls: None,
                        tool_call_id: Some(call.id.clone()),
                        name: Some(fn_name.clone()),
                    });
                }
                // Continue loop so model can process tool output
                continue;
            }
        }

        // Model did not request more tool calls; return final text response
        let final_text = assistant_message
            .content
            .clone()
            .unwrap_or_else(|| "(No response text provided)".to_string());
        messages.push(assistant_message);
        return Ok(final_text);
    }

    Err("Agent reached maximum reasoning turns without completing.".into())
}

pub async fn run_agent_session(
    config: AgentConfig,
    initial_prompt: Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("{}", "======================================================================".cyan().bold());
    println!("{}", "                   SEARCHUP AUTONOMOUS DATA AGENT                    ".bright_cyan().bold());
    println!("{}", "======================================================================".cyan().bold());
    println!("  {:<16} {}", "Target File:".bold().white(), config.file_path.display().to_string().cyan());
    println!("  {:<16} {}", "Model:".bold().white(), config.model.bright_yellow());
    println!("  {:<16} {}", "API Base:".bold().white(), config.api_base.dimmed());
    println!("  {:<16} {}", "Workers:".bold().white(), config.workers.to_string().green());
    println!("{}\n", "----------------------------------------------------------------------".dimmed());

    let client = reqwest::Client::new();
    let tools = get_agent_tools();

    let system_prompt = format!(
        "You are SearchUp Agent, a high-performance autonomous data cleaning and inspection copilot.\n\
        The user has loaded a CSV dataset at path: '{}'.\n\n\
        You have direct access to parallel dataset tools:\n\
        1. `profile_dataset`: Get schemas, inferred types (Integer, Float, Boolean, DateTime, String), null counts, and stats.\n\
        2. `search_missing_values`: Find exact coordinates of null/empty/NA values.\n\
        3. `search_outliers`: Find numeric anomalies using Tukey's IQR rule.\n\
        4. `fill_missing_value`: Repair missing values using 'mean', 'median', 'mode', or a literal value.\n\n\
        STRATEGY RULES:\n\
        - If asked to inspect or clean the dataset, first call `profile_dataset` or `search_missing_values`.\n\
        - Choose imputation strategies based on the column's inferred type (e.g., 'mean'/'median' for numbers, 'mode' or literal for strings).\n\
        - Keep your responses structured, clear, and informative.",
        config.file_path.display()
    );

    let mut messages: Vec<ChatMessage> = vec![ChatMessage {
        role: "system".to_string(),
        content: Some(system_prompt),
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }];

    if let Some(prompt) = initial_prompt {
        println!("{} {}\n", "User >".bold().bright_green(), prompt);
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: Some(prompt),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

        match run_agent_turn(&client, &config, &mut messages, &tools).await {
            Ok(response) => {
                println!("{} \n{}\n", "Agent >".bold().bright_cyan(), response);
            }
            Err(e) => {
                eprintln!("{}: {}", "Agent error".bright_red().bold(), e);
            }
        }
    } else {
        println!("{}", "Interactive mode enabled. Type your request or 'exit' / 'quit' to end.\n".dimmed());
        loop {
            print!("{} ", "searchup>".bold().bright_green());
            io::stdout().flush()?;

            let mut input = String::new();
            if io::stdin().read_line(&mut input)? == 0 {
                break;
            }

            let input = input.trim();
            if input.is_empty() {
                continue;
            }
            if input.eq_ignore_ascii_case("exit") || input.eq_ignore_ascii_case("quit") {
                println!("{}", "Goodbye!".bright_cyan());
                break;
            }

            messages.push(ChatMessage {
                role: "user".to_string(),
                content: Some(input.to_string()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            });

            match run_agent_turn(&client, &config, &mut messages, &tools).await {
                Ok(response) => {
                    println!("\n{} \n{}\n", "Agent >".bold().bright_cyan(), response);
                }
                Err(e) => {
                    eprintln!("{}: {}\n", "Agent error".bright_red().bold(), e);
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_agent_tools_definition() {
        let tools = get_agent_tools();
        assert_eq!(tools.len(), 4);
        let names: Vec<_> = tools.iter().map(|t| t.function.name.as_str()).collect();
        assert!(names.contains(&"profile_dataset"));
        assert!(names.contains(&"search_missing_values"));
        assert!(names.contains(&"search_outliers"));
        assert!(names.contains(&"fill_missing_value"));
    }

    #[test]
    fn test_execute_tool_profile() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(temp, "id,age,name").unwrap();
        writeln!(temp, "1,20,Alice").unwrap();
        writeln!(temp, "2,,Bob").unwrap();
        temp.flush().unwrap();

        let config = AgentConfig {
            file_path: temp.path().to_path_buf(),
            workers: 2,
            ..Default::default()
        };

        let result = execute_tool("profile_dataset", "{}", &config).expect("profile tool failed");
        assert!(result.contains("\"total_rows\": 2") || result.contains("\"total_rows\":2"));
        assert!(result.contains("age"));
    }

    #[test]
    fn test_execute_tool_search_missing() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(temp, "id,age,name").unwrap();
        writeln!(temp, "1,20,Alice").unwrap();
        writeln!(temp, "2,,Bob").unwrap();
        temp.flush().unwrap();

        let config = AgentConfig {
            file_path: temp.path().to_path_buf(),
            workers: 2,
            ..Default::default()
        };

        let result = execute_tool("search_missing_values", r#"{"limit": 5}"#, &config)
            .expect("search_missing_values failed");
        assert!(result.contains("\"total_matches\": 1") || result.contains("\"total_matches\":1"));
    }

    #[test]
    fn test_execute_tool_fill() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(temp, "id,age,name").unwrap();
        writeln!(temp, "1,20,Alice").unwrap();
        writeln!(temp, "2,40,Bob").unwrap();
        writeln!(temp, "3,,Charlie").unwrap();
        temp.flush().unwrap();

        let config = AgentConfig {
            file_path: temp.path().to_path_buf(),
            workers: 2,
            ..Default::default()
        };

        let result = execute_tool(
            "fill_missing_value",
            r#"{"column": "age", "value": "mean", "in_place": true}"#,
            &config,
        )
        .expect("fill_missing_value failed");

        assert!(result.contains("success"));
        assert!(result.contains("\"cells_updated\": 1") || result.contains("\"cells_updated\":1"));
    }

    #[test]
    fn test_extract_fallback_tool_calls_json() {
        let tools = get_agent_tools();
        let raw_json = r#"{"name": "search_missing_values", "arguments": {"limit": 20}}"#;
        let calls = extract_fallback_tool_calls(raw_json, &tools).expect("should extract call");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "search_missing_values");
        assert!(calls[0].function.arguments.contains("20"));
    }

    #[test]
    fn test_extract_fallback_tool_calls_double_brace() {
        let tools = get_agent_tools();
        let raw_json = r#"{"name": "search_missing_values", "arguments": {{"limit": 20}}}"#;
        let calls = extract_fallback_tool_calls(raw_json, &tools).expect("should extract call");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "search_missing_values");
        assert!(calls[0].function.arguments.contains("20"));
    }

    #[test]
    fn test_extract_fallback_tool_calls_code_fence() {
        let tools = get_agent_tools();
        let markdown = "```json\n{\"name\": \"search_outliers\", \"arguments\": {\"limit\": 10}}\n```";
        let calls = extract_fallback_tool_calls(markdown, &tools).expect("should extract call");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "search_outliers");
    }

    #[test]
    fn test_extract_fallback_tool_calls_ignore_regular_text() {
        let tools = get_agent_tools();
        let text = "Here is an explanation of the dataset. Everything looks clean.";
        let calls = extract_fallback_tool_calls(text, &tools);
        assert!(calls.is_none());
    }
}
