//! `cyberdeck recon` — direct-mode CLI for the seven Recon primitives.
//!
//! Each subcommand is a 1:1 mirror of `cyberdeck_intel::recon::*::run`.
//! We don't add a daemon `Method` for these — the primitives are sync
//! (`dig` / `whois` / `openssl s_client` / local CSV / `ureq`), short-
//! lived, and never carry state; the cost of a JSON-RPC round-trip
//! outweighs the benefit. The CLI hits the same code path the screen
//! hits so the two stay identical (and `cyberdeck recon dns example.com`
//! is a useful debug aid when the Recon screen "looks weird").
//!
//! Subcommand design notes:
//!
//!   * One verb per `ReconTab`. The verb names are lowercase to match
//!     the screen's tab labels (`dns`, `whois`, `ip`, `ssl`, `cve`,
//!     `crypto`, `sanctions`).
//!   * Each arm takes a positional `<query>` so scripts can pipe
//!     through `xargs` — same pattern as `cyberdeck city locate`.
//!   * No `--json` projection; the primitives already emit JSON
//!     (or shaped text) directly. The CLI pipes the body to stdout
//!     verbatim, exit 0 on success, exit 1 on failure (so a
//!     `cyberdeck recon ip 10.0.0.1` produces rc=1 to flag the
//!     SSRF block).

use anyhow::Result;
use clap::Subcommand;

use crate::output::OutputMode;
use cyberdeck_intel::recon::ReconTab;

#[derive(Debug, Subcommand)]
pub enum ReconCmd {
    /// DNS A/AAAA lookup (shells out to `dig +short`).
    Dns {
        /// Hostname to resolve (e.g. `example.com`).
        query: String,
    },
    /// WHOIS registry lookup (shells out to `whois`).
    Whois {
        /// Domain or ASN to look up.
        query: String,
    },
    /// IP-geolocation via ip-api.com (SSRF-gated against
    /// loopback / RFC1918 / link-local).
    Ip {
        /// IPv4/IPv6 literal or hostname.
        query: String,
    },
    /// TLS handshake inspection (shells out to `openssl s_client`).
    Ssl {
        /// `<host>` or `<host:port>` (default port 443).
        query: String,
    },
    /// CVE search via the bundled offline fixture.
    Cve {
        /// Substring to match against vendor keywords in the fixture.
        query: String,
    },
    /// Crypto-address risk lookup (bundled fixture).
    Crypto {
        /// Substring against address / category / note.
        query: String,
    },
    /// Sanctions search (bundled OFAC SDN mirror).
    Sanctions {
        /// Substring against name / remarks / aka.
        query: String,
    },
}

/// Helper used by both the dispatcher (`lib.rs`) and the test suite
/// to map a `ReconTab` to the right `recon::<arm>::run` function.
/// Pulled out so the match arm doesn't get duplicated across the
/// RunAction dispatch and the test harness.
pub fn run_arm(tab: ReconTab, query: &str) -> Result<String> {
    match tab {
        ReconTab::Dns => cyberdeck_intel::recon::dns::run(query),
        ReconTab::Whois => cyberdeck_intel::recon::whois::run(query),
        ReconTab::Ip => cyberdeck_intel::recon::ip::run(query),
        ReconTab::Ssl => cyberdeck_intel::recon::ssl::run(query),
        ReconTab::Cve => cyberdeck_intel::recon::cve::run(query),
        ReconTab::Crypto => cyberdeck_intel::recon::crypto::run(query),
        ReconTab::Sanctions => cyberdeck_intel::recon::sanctions::run(query),
    }
}

pub fn run(cmd: ReconCmd, mode: OutputMode) -> Result<i32> {
    let (tab, query) = match cmd {
        ReconCmd::Dns { query } => (ReconTab::Dns, query),
        ReconCmd::Whois { query } => (ReconTab::Whois, query),
        ReconCmd::Ip { query } => (ReconTab::Ip, query),
        ReconCmd::Ssl { query } => (ReconTab::Ssl, query),
        ReconCmd::Cve { query } => (ReconTab::Cve, query),
        ReconCmd::Crypto { query } => (ReconTab::Crypto, query),
        ReconCmd::Sanctions { query } => (ReconTab::Sanctions, query),
    };
    match run_arm(tab, &query) {
        Ok(body) => {
            // Emit the body verbatim — the primitives already shape
            // it (JSON for json-shaped arms, plain text for shell-out
            // arms). JSON output mode picks up the `mode` flag; we
            // only re-shape when the user asked for `--json` and
            // the body isn't already JSON.
            if matches!(mode, OutputMode::Json) && !body.trim_start().starts_with('{') {
                crate::output::print(mode, &serde_json::json!({ "output": body })).map(|_| 0)
            } else {
                println!("{body}");
                Ok(0)
            }
        }
        Err(e) => {
            // Same "log + continue" contract the other verbs use:
            // structured error envelope, exit 0. A shell pipeline
            // that just wants to know "did this work" can grep the
            // error code; an interactive caller reads the message.
            crate::output::print(
                mode,
                &serde_json::json!({
                    "ok": false,
                    "error": { "message": e.to_string() },
                    "tab": tab.label(),
                    "query": query,
                }),
            )
            .map(|_| 1)
        }
    }
}
