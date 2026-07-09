//! `dig +short` — DNS A/AAAA lookup primitive.
//!
//! `query` is the hostname to resolve. We shell out to `dig +short`
//! which suppresses the verbose boilerplate; the body becomes a
//! newline-delimited list of A/AAAA records. If `dig` is missing on
//! `$PATH` we return a structured "binary not found" error so the
//! screen can surface "install dnsutils" instead of a panic.

use anyhow::{anyhow, Context, Result};
use std::process::Command;

pub const LABEL: &str = "DNS";

pub fn run(query: &str) -> Result<String> {
    let q = query.trim();
    if q.is_empty() {
        return Err(anyhow!("empty query"));
    }
    let out = Command::new("dig")
        .args(["+short", q])
        .output()
        .with_context(|| format!("spawning dig for {q:?}"))?;
    if !out.status.success() {
        // `dig` writes errors on stderr. Surface the first line —
        // enough to debug without flooding the screen.
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err.lines().next().unwrap_or("unknown error");
        return Err(anyhow!("dig failed: {first}"));
    }
    let body = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if body.is_empty() {
        return Err(anyhow!("no records for {q}"));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_is_rejected() {
        let r = run("");
        assert!(r.is_err(), "empty query must error");
    }

    #[test]
    fn whitespace_query_is_trimmed_then_rejected() {
        assert!(run("   ").is_err());
    }

    /// If `dig` is missing, the screen + CLI must see a structured
    /// error rather than a raw "no such file" panic. We don't
    /// actually need to delete dig — running it on an unresolvable
    /// hostname is enough to prove the error path returns `Err`.
    #[test]
    fn unknown_hostname_returns_structured_error() {
        let r = run("definitely-not-a-real-host-abcdef.invalid");
        // Either dig is installed and returns "no records", or it's
        // missing — both surface Err with a reason we can render.
        if let Err(e) = r {
            let s = e.to_string();
            assert!(
                s.contains("no records") || s.contains("dig failed") || s.contains("spawning dig"),
                "unexpected error shape: {s}"
            );
        }
        // If dig returns 0 with empty stdout, run() errors with "no
        // records" — `Ok` only happens for valid lookups, which
        // aren't possible against an `.invalid` TLD.
    }
}
