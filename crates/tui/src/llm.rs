//! llama-server sidecar lifecycle + streaming chat client.
//!
//! Designed for Qwen3-1.7B Q4_K_M running on the ClockworkPi uConsole CM4.
//! The model loads ONCE at TUI boot and stays warm until exit. All HTTP is
//! async (reqwest + chunk()); the tokio event loop is never blocked.

use std::time::Duration;

use tokio::process::Command;
use tokio::sync::mpsc;

use crate::app::action::Action;
use crate::app::live_data::{AiMessage, AiRole};
use crate::prefs::Prefs;

const LLAMA_PORT: u16 = 8081;
const LLAMA_URL: &str = "http://127.0.0.1:8081";

const SYSTEM_PROMPT: &str = "You are a CyberDeck field AI assistant embedded in a terminal on a \
    ClockworkPi uConsole CM4. Be concise and practical — the screen is small. \
    Use code blocks for shell commands. Think through complex problems using <think> tags.";

pub struct LlamaSidecar {
    child: tokio::process::Child,
}

/// Spawn llama-server with the discovered model. Returns None if no model
/// found or if llama-server binary is missing.
pub async fn spawn_sidecar(prefs: &Prefs) -> Option<LlamaSidecar> {
    let model_path = find_model_path(prefs)?;
    let child = Command::new("llama-server")
        .args([
            "--model", &model_path,
            "-c", "4096",
            "-t", "4",
            "--port", &LLAMA_PORT.to_string(),
            "-ngl", "0",  // CPU-only on CM4 (no CUDA)
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    Some(LlamaSidecar { child })
}

/// Graceful SIGTERM → 2s wait → SIGKILL.
pub async fn kill_sidecar(sidecar: &mut LlamaSidecar) {
    let _ = sidecar.child.start_kill();
    match tokio::time::timeout(Duration::from_secs(2), sidecar.child.wait()).await {
        Ok(_) => {}
        Err(_) => { let _ = sidecar.child.kill().await; }
    }
}

/// Poll /health every 1s for up to 30s. Sends LlamaReady or LlamaDown.
pub fn spawn_health_poll(tx: mpsc::Sender<Action>) {
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let ok = client
                .get(format!("{LLAMA_URL}/health"))
                .timeout(Duration::from_secs(1))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if ok {
                tx.send(Action::LlamaReady).await.ok();
                return;
            }
        }
        tx.send(Action::LlamaDown).await.ok();
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

    let body = serde_json::json!({
        "model": "qwen3",
        "messages": json_msgs,
        "stream": true,
        "temperature": 0.7,
    });

    let client = reqwest::Client::new();
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

    let mut resp = resp;
    let mut sse_buf = String::new();
    let mut in_think = false;
    let mut pending = String::new();

    loop {
        match resp.chunk().await {
            Ok(Some(bytes)) => {
                sse_buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(nl) = sse_buf.find('\n') {
                    let line = sse_buf[..nl].trim_end_matches('\r').to_string();
                    sse_buf = sse_buf[nl + 1..].to_string();
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            flush_pending(&mut pending, in_think, &tx).await;
                            tx.send(Action::AiDone).await.ok();
                            return;
                        }
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(tok) = v["choices"][0]["delta"]["content"].as_str() {
                                if !tok.is_empty() {
                                    route_token(tok, &mut in_think, &mut pending, &tx).await;
                                }
                            }
                        }
                    }
                }
            }
            Ok(None) => break, // EOF
            Err(e) => {
                tracing::warn!("llama-server stream error: {e}");
                break;
            }
        }
    }

    flush_pending(&mut pending, in_think, &tx).await;
    tx.send(Action::AiDone).await.ok();
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
