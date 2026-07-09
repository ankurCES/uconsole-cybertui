//! Standalone `cyberdeck-web` binary: same as the TUI's embedded mode, but
//! without a TUI. Useful for headless servers, Docker containers, or for
//! simply exposing a cyberdeck's controls from another machine on the LAN.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cyberdeck_web::auth::Token;
use cyberdeck_web::run::standalone::StandaloneLive;
use cyberdeck_web::run_with;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Tiny arg parser: bind addr (positional, optional), then `--token-file PATH`.
    let mut bind: Option<String> = None;
    let mut token_file: Option<PathBuf> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--token-file" => {
                token_file = it.next().map(PathBuf::from);
            }
            "--help" | "-h" => {
                eprintln!("usage: cyberdeck-web [BIND_ADDR] [--token-file PATH]");
                eprintln!("  BIND_ADDR   default: 127.0.0.1:7878");
                eprintln!("  --token-file  read bearer token from PATH (installer pins it)");
                std::process::exit(0);
            }
            other if !other.starts_with("--") && bind.is_none() => {
                bind = Some(other.to_string());
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let bind = bind.unwrap_or_else(|| cyberdeck_web::run::DEFAULT_BIND.to_string());

    // If a token file is given and exists, use it; otherwise generate a fresh
    // token for this run (and warn so the operator knows it won't persist).
    let token: Option<Token> = match token_file.as_deref() {
        Some(p) if p.exists() => match Token::from_file(p) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!(
                    "cyberdeck-web: failed to read token from {}: {e}",
                    p.display()
                );
                std::process::exit(1);
            }
        },
        Some(p) => {
            eprintln!(
                "cyberdeck-web: token file {} not found; generating fresh token",
                p.display()
            );
            None
        }
        None => None,
    };

    let live = Arc::new(StandaloneLive::default());
    live.spawn_refreshers();

    // Refresh the upgradable list less often — apt is slow.
    let me = live.clone();
    tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_secs(60));
        loop {
            t.tick().await;
            if let Ok(v) = cyberdeck_core::packages::upgradable().await {
                *me.upgradable.write().await = v;
            }
        }
    });

    run_with(&bind, live, None, token).await
}
