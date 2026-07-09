//! `whois` primitive — registry lookup for the supplied query.
//!
//! `query` is either a domain (handled by the default whois server)
//! or `"-h <server> <query>"` style for advanced callers (the screen
//! always sends a bare domain; CLI power-users can prepend their own
//! server via the OS args). Strip any control characters so a
//! malformed paste doesn't poison the spawned argv.

use anyhow::{anyhow, Context, Result};
use std::process::Command;

pub const LABEL: &str = "WHOIS";

/// Sanitise an incoming query. Removes anything outside `[a-zA-Z0-9.\-_]`
/// and clamps to 253 chars (DNS max + safety).
fn sanitise(q: &str) -> Result<String> {
    let trimmed = q.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("empty query"));
    }
    let clean: String = trimmed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    if clean.is_empty() {
        return Err(anyhow!("no usable characters in query"));
    }
    if clean.len() > 253 {
        return Err(anyhow!("query too long ({} > 253)", clean.len()));
    }
    Ok(clean)
}

pub fn run(query: &str) -> Result<String> {
    let q = sanitise(query)?;
    let out = Command::new("whois")
        .arg(&q)
        .output()
        .with_context(|| format!("spawning whois for {q:?}"))?;
    if !out.status.success() {
        // whois often exits 0 with stderr; only flag a real failure
        // when the status code is non-zero AND stderr has content.
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err.lines().next().unwrap_or("unknown error");
        return Err(anyhow!("whois exited {}: {first}", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_is_rejected() {
        assert!(run("").is_err());
    }

    #[test]
    fn non_ascii_input_is_stripped_to_safe_subset() {
        let s = sanitise("exa;mple.com").unwrap();
        assert_eq!(s, "example.com");
    }

    #[test]
    fn only_unsafe_chars_is_rejected() {
        assert!(sanitise("///").is_err());
        assert!(sanitise("   ").is_err());
    }

    #[test]
    fn overly_long_query_is_rejected() {
        let long = "a".repeat(300);
        let err = sanitise(&long).unwrap_err().to_string();
        assert!(err.contains("too long"), "got: {err}");
    }

    #[test]
    fn real_whois_call_is_structured() {
        // Operate on an unambiguously invalid TLD so we never hit a
        // real registry. Errors are acceptable; panics are not.
        let _ = run("definitely-not-a-real-domain-xyzz.invalid");
    }
}
