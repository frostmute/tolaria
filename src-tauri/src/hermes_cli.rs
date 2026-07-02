use crate::ai_agents::{AiAgentAvailability, AiAgentStreamEvent};
use crate::cli_agent_runtime::AgentStreamRequest;
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::process::{ChildStderr, ChildStdout, Stdio};

struct HermesError<'a> {
    stderr_output: &'a str,
    status: String,
}

pub fn check_cli() -> AiAgentAvailability {
    crate::hermes_discovery::check_cli()
}


use serde_json::json;

pub fn run_agent_stream<F>(request: AgentStreamRequest, mut emit: F) -> Result<String, String>
where
    F: FnMut(AiAgentStreamEvent),
{
    let binary = crate::hermes_discovery::find_binary()?;
    let prompt =
        crate::cli_agent_runtime::build_prompt(&request.message, request.system_prompt.as_deref());

    let mut child = spawn_hermes_process(&binary, Path::new(&request.vault_path))?;
    
    let mut stdin = child.stdin.take().ok_or("No stdin handle")?;
    let stderr_handle = read_stderr_async(child.stderr.take().ok_or("No stderr handle")?);

    let session_id = generate_session_id();
    emit(AiAgentStreamEvent::Init {
        session_id: session_id.clone(),
    });

    // Send ACP initialize request
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });
    let _ = writeln!(stdin, "{}", init_req);

    // Send ACP session/create request
    let create_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/create",
        "params": {
            "message": prompt,
            "session_id": session_id
        }
    });
    let _ = writeln!(stdin, "{}", create_req);

    stream_stdout(child.stdout.take().ok_or("No stdout handle")?, &mut emit);

    let stderr_output = stderr_handle.join().unwrap_or_default();
    let status = child.wait().map_err(|e| format!("Wait failed: {e}"))?;
    if !status.success() {
        emit(AiAgentStreamEvent::Error {
            message: format_hermes_error(HermesError {
                stderr_output: &stderr_output,
                status: status.to_string(),
            }),
        });
    }

    emit(AiAgentStreamEvent::Done);
    Ok(session_id)
}

// Hermes -q passes the prompt as a CLI argument; no stdin needed.
fn spawn_hermes_process(
    binary: &Path,
    vault_path: &Path,
) -> Result<std::process::Child, String> {
    let mut command = crate::hidden_command(binary);
    crate::cli_agent_runtime::configure_agent_command_environment(&mut command, binary);
    command
        .arg("acp")
        .arg("--accept-hooks")
        .current_dir(vault_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
        .spawn()
        .map_err(|e| format!("Failed to spawn hermes: {e}"))
}

fn generate_session_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("hermes-{}-{}", std::process::id(), ts)
}


fn stream_stdout<F>(stdout: ChildStdout, emit: &mut F)
where
    F: FnMut(AiAgentStreamEvent),
{
    let reader = std::io::BufReader::new(stdout);

    for line in reader.lines() {
        match line {
            Ok(l) if !l.is_empty() => {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&l) {
                    if json["method"] == "session/update" {
                        if let Some(params) = json.get("params") {
                            if let Some(update) = params.get("update") {
                                let session_update = update["sessionUpdate"].as_str().unwrap_or("");
                                match session_update {
                                    "agent_message_chunk" => {
                                        if let Some(content) = update.get("content") {
                                            if let Some(text) = content["text"].as_str() {
                                                emit(AiAgentStreamEvent::TextDelta { text: text.to_string() });
                                            }
                                        }
                                    }
                                    "agent_thought_chunk" => {
                                        if let Some(content) = update.get("content") {
                                            if let Some(text) = content["text"].as_str() {
                                                emit(AiAgentStreamEvent::ThinkingDelta { text: text.to_string() });
                                            }
                                        }
                                    }
                                    "tool_call_update" => {
                                        let tool_id = update["toolCallId"].as_str().unwrap_or("").to_string();
                                        let tool_name = update["toolName"].as_str().unwrap_or("").to_string();
                                        let input = update.get("input").and_then(|i| i.as_str()).map(|s| s.to_string());
                                        emit(AiAgentStreamEvent::ToolStart { tool_name, tool_id, input });
                                    }
                                    "tool_call_result" => {
                                        let tool_id = update["toolCallId"].as_str().unwrap_or("").to_string();
                                        let output = update.get("output").and_then(|o| o.as_str()).map(|s| s.to_string());
                                        emit(AiAgentStreamEvent::ToolDone { tool_id, output });
                                    }
                                    _ => {}
                                }
                            }
                        }
                    } else if json["method"] == "session/done" {
                        break;
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                emit(AiAgentStreamEvent::Error {
                    message: format!("Read error: {e}"),
                });
                break;
            }
        }
    }
}

fn read_stderr_async(mut stderr: ChildStderr) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut output = String::new();
        let _ = stderr.read_to_string(&mut output);
        output
    })
}



fn format_hermes_error(error: HermesError<'_>) -> String {
    if is_auth_error(error.stderr_output) {
        return "Hermes is not authenticated. Run `hermes` in your terminal to sign in.".into();
    }
    if error.stderr_output.trim().is_empty() {
        format!("hermes exited with status {}", error.status)
    } else {
        error
            .stderr_output
            .lines()
            .take(3)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn is_auth_error(stderr_output: &str) -> bool {
    let lower = stderr_output.to_ascii_lowercase();
    ["auth", "login", "token", "unauthorized"]
        .iter()
        .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    

    #[test]
    fn format_hermes_error_detects_auth_errors() {
        let result = format_hermes_error(HermesError {
            stderr_output: "Error: auth token expired",
            status: "1".into(),
        });
        assert!(result.contains("hermes"));
        assert!(result.contains("sign in"));
    }

    #[test]
    fn format_hermes_error_returns_status_for_empty_stderr() {
        let result = format_hermes_error(HermesError {
            stderr_output: "",
            status: "1".into(),
        });
        assert!(result.contains("status 1"));
    }
}
