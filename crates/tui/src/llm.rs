//! llama-server sidecar lifecycle + streaming chat client.
//!
//! Designed for MiniCPM5-1B Q4_K_M running on the ClockworkPi uConsole CM4.
//! The model loads ONCE at TUI boot and stays warm until exit. All HTTP is
//! async (reqwest + chunk()); the tokio event loop is never blocked.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, RwLock};

use crate::app::action::Action;
use crate::app::live_data::{AiMessage, AiRole};
use crate::prefs::Prefs;

const LLAMA_PORT: u16 = 8081;
const LLAMA_URL: &str = "http://127.0.0.1:8081";

const SYSTEM_PROMPT: &str = "You are a CyberDeck field AI assistant embedded in a terminal on a \
    ClockworkPi uConsole CM4. Be concise and practical — the screen is small. \
    Use code blocks for shell commands.";

pub struct LlamaSidecar {
    child: tokio::process::Child,
    pub stderr_tail: Arc<RwLock<Vec<String>>>,
}

/// Spawn llama-server with the discovered model. Returns None if no model
/// found or if llama-server binary is missing.
pub async fn spawn_sidecar(prefs: &Prefs) -> Option<LlamaSidecar> {
    let model_path = find_model_path(prefs)?;
    let mut child = Command::new("llama-server")
        .args([
            "--model", &model_path,
            "--host", "127.0.0.1",
            "--port", &LLAMA_PORT.to_string(),
            "-c", "4096",
            "-t", "4",
            "-ngl", "0",
            "--jinja",
            "--no-webui",
        ])
        .env("LLAMA_ARG_NO_DISPLAY_PROMPT", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    // Capture last 30 lines of stderr for error reporting.
    let stderr_tail: Arc<RwLock<Vec<String>>> = Arc::new(RwLock::new(Vec::new()));
    if let Some(stderr) = child.stderr.take() {
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::debug!(target: "llama", "{line}");
                let mut buf = tail.write().await;
                buf.push(line);
                if buf.len() > 30 { buf.remove(0); }
            }
        });
    }

    Some(LlamaSidecar { child, stderr_tail })
}

/// Graceful SIGTERM → 2s wait → SIGKILL.
pub async fn kill_sidecar(sidecar: &mut LlamaSidecar) {
    let _ = sidecar.child.start_kill();
    match tokio::time::timeout(Duration::from_secs(2), sidecar.child.wait()).await {
        Ok(_) => {}
        Err(_) => { let _ = sidecar.child.kill().await; }
    }
}

/// Poll /health every 500ms for up to 90s. Detects early process exit.
/// Sends LlamaReady or LlamaDown (with stderr tail on failure).
pub fn spawn_health_poll(
    tx: mpsc::Sender<Action>,
    sidecar: &LlamaSidecar,
) {
    let stderr_tail = sidecar.stderr_tail.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        for _ in 0..180 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let ok = client
                .get(format!("{LLAMA_URL}/health"))
                .timeout(Duration::from_secs(2))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if ok {
                tx.send(Action::LlamaReady).await.ok();
                return;
            }
        }
        // Timeout — report stderr tail so user knows why
        let tail = stderr_tail.read().await;
        let detail = if tail.is_empty() {
            "timed out after 90s".into()
        } else {
            tail.last().cloned().unwrap_or_default()
        };
        tx.send(Action::LlamaFailed(detail)).await.ok();
    });
}

/// Check prefs.ai_model_path, then scan ~/.cyberdeck/models/ for first .gguf.
pub fn find_model_path(prefs: &Prefs) -> Option<String> {
    if let Some(ref p) = prefs.ai_model_path {
        if std::path::Path::new(p).exists() {
            return Some(p.clone());
        }
    }
    let dir = dirs::home_dir()?.join(".cyberdeck/models");
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| {
            e.path()
                .extension()
                .map(|x| x == "gguf")
                .unwrap_or(false)
        })
        .map(|e| e.path().to_string_lossy().into_owned())
}

/// POST a chat completion request to llama-server and stream the response
/// back through the Action channel. `msgs` is a snapshot of the full
/// conversation history (all messages except the current streaming one).
///
/// Supports tool calling: if the model returns tool_calls, we execute them
/// and re-request with the results (up to MAX_TOOL_ROUNDS to prevent loops).
pub async fn stream_chat(msgs: Vec<AiMessage>, tx: mpsc::Sender<Action>) {
    let mut json_msgs = vec![serde_json::json!({
        "role": "system",
        "content": SYSTEM_PROMPT,
    })];
    for msg in &msgs {
        let role = match msg.role {
            AiRole::User => "user",
            AiRole::Assistant => "assistant",
        };
        json_msgs.push(serde_json::json!({
            "role": role,
            "content": msg.full_text(),
        }));
    }

    let tools = crate::tools::tool_definitions();
    let client = reqwest::Client::new();

    // Tool-call loop: stream → maybe tool calls → re-request
    const MAX_TOOL_ROUNDS: usize = 5;
    for _round in 0..MAX_TOOL_ROUNDS {
        let body = serde_json::json!({
            "model": "minicpm5",
            "messages": json_msgs,
            "stream": true,
            "temperature": 0.6,
            "top_p": 0.95,
            "max_tokens": 1024,
            "tools": tools,
            "chat_template_kwargs": { "enable_thinking": true },
        });

        let resp = match client
            .post(format!("{LLAMA_URL}/v1/chat/completions"))
            .json(&body)
            .timeout(Duration::from_secs(120))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("llama-server request failed: {e}");
                tx.send(Action::AiToken(format!("⚠ Connection failed: {e}"))).await.ok();
                tx.send(Action::AiDone).await.ok();
                return;
            }
        };

        let stream_result = stream_one_response(resp, &tx).await;

        match stream_result {
            StreamResult::Done => {
                tx.send(Action::AiDone).await.ok();
                return;
            }
            StreamResult::ToolCalls(calls) => {
                // Append assistant message with tool_calls
                json_msgs.push(serde_json::json!({
                    "role": "assistant",
                    "tool_calls": calls.iter().map(|tc| serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": { "name": tc.name, "arguments": tc.arguments }
                    })).collect::<Vec<_>>(),
                }));

                // Execute each tool and append results
                for tc in &calls {
                    let args: serde_json::Value = serde_json::from_str(&tc.arguments)
                        .unwrap_or(serde_json::json!({}));
                    let result = tokio::task::spawn_blocking({
                        let name = tc.name.clone();
                        let args = args.clone();
                        move || crate::tools::execute_tool(&name, &args)
                    }).await.unwrap_or_else(|_| "tool execution panicked".into());

                    let log = crate::tools::tool_log_line(&tc.name, &args, &result);
                    tx.send(Action::AiToolLog(log)).await.ok();

                    json_msgs.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tc.id,
                        "content": result,
                    }));
                }
                // Loop back for the model's next response
            }
        }
    }

    tx.send(Action::AiToken("(tool call limit reached)".into())).await.ok();
    tx.send(Action::AiDone).await.ok();
}

struct ToolCall {
    id: String,
    name: String,
    arguments: String,
}

enum StreamResult {
    Done,
    ToolCalls(Vec<ToolCall>),
}

/// Stream a single response from llama-server. Returns whether the model
/// finished with content (Done) or wants to call tools (ToolCalls).
async fn stream_one_response(
    mut resp: reqwest::Response,
    tx: &mpsc::Sender<Action>,
) -> StreamResult {
    let mut sse_buf = String::new();
    let mut in_think = false;
    let mut pending = String::new();

    // Accumulate tool calls across SSE chunks
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut finish_reason = String::new();

    loop {
        match resp.chunk().await {
            Ok(Some(bytes)) => {
                sse_buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(nl) = sse_buf.find('\n') {
                    let line = sse_buf[..nl].trim_end_matches('\r').to_string();
                    sse_buf = sse_buf[nl + 1..].to_string();
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            flush_pending(&mut pending, in_think, tx).await;
                            if !tool_calls.is_empty() {
                                return StreamResult::ToolCalls(tool_calls);
                            }
                            return StreamResult::Done;
                        }
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                            let choice = &v["choices"][0];
                            let delta = &choice["delta"];

                            // Check finish_reason
                            if let Some(fr) = choice["finish_reason"].as_str() {
                                finish_reason = fr.to_string();
                            }

                            // reasoning_content (--jinja thinking)
                            if let Some(rc) = delta["reasoning_content"].as_str() {
                                if !rc.is_empty() {
                                    tx.send(Action::AiThinkToken(rc.to_string())).await.ok();
                                }
                            }

                            // content tokens
                            if let Some(tok) = delta["content"].as_str() {
                                if !tok.is_empty() {
                                    route_token(tok, &mut in_think, &mut pending, tx).await;
                                }
                            }

                            // tool_calls — streamed incrementally
                            if let Some(tcs) = delta["tool_calls"].as_array() {
                                for tc_delta in tcs {
                                    let idx = tc_delta["index"].as_u64().unwrap_or(0) as usize;
                                    while tool_calls.len() <= idx {
                                        tool_calls.push(ToolCall {
                                            id: String::new(),
                                            name: String::new(),
                                            arguments: String::new(),
                                        });
                                    }
                                    if let Some(id) = tc_delta["id"].as_str() {
                                        tool_calls[idx].id = id.to_string();
                                    }
                                    if let Some(func) = tc_delta["function"].as_object() {
                                        if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                                            tool_calls[idx].name = name.to_string();
                                        }
                                        if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                                            tool_calls[idx].arguments.push_str(args);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                tracing::warn!("llama-server stream error: {e}");
                break;
            }
        }
    }

    flush_pending(&mut pending, in_think, tx).await;

    if !tool_calls.is_empty() || finish_reason == "tool_calls" {
        // Fill in missing tool call IDs
        for (i, tc) in tool_calls.iter_mut().enumerate() {
            if tc.id.is_empty() {
                tc.id = format!("call_{i}");
            }
        }
        StreamResult::ToolCalls(tool_calls)
    } else {
        StreamResult::Done
    }
}

async fn flush_pending(pending: &mut String, in_think: bool, tx: &mpsc::Sender<Action>) {
    if !pending.is_empty() {
        let act = if in_think {
            Action::AiThinkToken(std::mem::take(pending))
        } else {
            Action::AiToken(std::mem::take(pending))
        };
        tx.send(act).await.ok();
    }
}

/// Route a raw SSE content token through the <think>...</think> state machine.
/// `pending` buffers chars that might be part of a tag boundary across tokens.
async fn route_token(
    tok: &str,
    in_think: &mut bool,
    pending: &mut String,
    tx: &mpsc::Sender<Action>,
) {
    pending.push_str(tok);

    loop {
        if !*in_think {
            if let Some(p) = pending.find("<think>") {
                // emit text before the tag
                if p > 0 {
                    tx.send(Action::AiToken(pending[..p].to_string())).await.ok();
                }
                *pending = pending[p + 7..].to_string();
                *in_think = true;
            } else if let Some(lt) = pending.find('<') {
                // emit safe prefix before '<', then check if rest is a tag prefix
                if lt > 0 {
                    tx.send(Action::AiToken(pending[..lt].to_string())).await.ok();
                    *pending = pending[lt..].to_string();
                }
                if "<think>".starts_with(pending.as_str()) {
                    break; // buffer partial tag, wait for next token
                } else {
                    tx.send(Action::AiToken(std::mem::take(pending))).await.ok();
                    break;
                }
            } else {
                if !pending.is_empty() {
                    tx.send(Action::AiToken(std::mem::take(pending))).await.ok();
                }
                break;
            }
        } else {
            // mirror logic for </think>
            if let Some(p) = pending.find("</think>") {
                if p > 0 {
                    tx.send(Action::AiThinkToken(pending[..p].to_string())).await.ok();
                }
                *pending = pending[p + 8..].to_string();
                *in_think = false;
            } else if let Some(lt) = pending.find('<') {
                if lt > 0 {
                    tx.send(Action::AiThinkToken(pending[..lt].to_string())).await.ok();
                    *pending = pending[lt..].to_string();
                }
                if "</think>".starts_with(pending.as_str()) {
                    break;
                } else {
                    tx.send(Action::AiThinkToken(std::mem::take(pending))).await.ok();
                    break;
                }
            } else {
                if !pending.is_empty() {
                    tx.send(Action::AiThinkToken(std::mem::take(pending))).await.ok();
                }
                break;
            }
        }
    }
}
