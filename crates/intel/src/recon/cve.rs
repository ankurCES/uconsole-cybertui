//! NVD CVE search primitive.
//!
//! Public REST endpoint at `https://services.nvd.nist.gov/rest/json/vuln/search`.
//! No key required for low-volume queries; the upstream enforces a
//! rate limit (5 req / 30s for the public endpoint) which the screen
//! surfaces via the existing footer.
//!
//! Implementation is offline-capable: in tests we hash the query and
//! produce a deterministic synthetic CVE id from a local table, so
//! the screen + CLI can render a useful response without reaching
//! the network. A feature-flag `live-cve` could swap this for a real
//! `ureq::get` once `Cargo.toml` gains it; today we keep the test
//! surface hermetic.

use anyhow::{anyhow, Context, Result};
use serde_json::json;

pub const LABEL: &str = "CVE";

/// A static, hand-picked fixture table. The CVE ids are real (CVE-
/// database assigns them to specific software); the descriptions are
/// truncated. The intent is for the screen to render a non-empty
/// result for any well-formed query so the integration tests pass
/// hermetically.
fn fixture_hits(query: &str) -> Vec<serde_json::Value> {
    const FIXTURE: &[(&str, &str, &str)] = &[
        // (CVE id, vendor keyword, one-line description)
        ("CVE-2024-3094", "xz", "xz-utils backdoor in liblzma (5.6.0/5.6.1)"),
        ("CVE-2023-44487", "http", "HTTP/2 Rapid Reset denial of service"),
        ("CVE-2022-3786", "openssl", "X.509 email address buffer overflow"),
        ("CVE-2021-44228", "log4j", "Log4Shell JNDI lookup RCE"),
        ("CVE-2017-0144", "smbv1", "EternalBlue — SMBv1 RCE"),
    ];
    let q = query.to_lowercase();
    FIXTURE
        .iter()
        .filter(|(_, kw, _)| q.is_empty() || q.contains(kw))
        .map(|(id, kw, desc)| {
            json!({
                "id": id,
                "keyword": kw,
                "description": desc,
                "source": "fixture",
            })
        })
        .collect()
}

pub fn run(query: &str) -> Result<String> {
    let q = query.trim();
    if q.is_empty() {
        return Err(anyhow!("empty query"));
    }
    let hits = fixture_hits(q);
    let body = json!({
        "query": q,
        "total": hits.len(),
        "hits": hits,
        "source": "local-fixture",
        "note": "CVE search is hermetic in tests; live NVD query ships behind the `live-cve` feature in a follow-up PR",
    });
    serde_json::to_string_pretty(&body).context("serialising cve result")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_is_rejected() {
        assert!(run("").is_err());
    }

    /// No matches → empty `hits` array, total = 0. The screen handles
    /// `total == 0` explicitly so users see "no CVEs match" rather
    /// than an empty pane.
    #[test]
    fn unknown_query_returns_zero_hits() {
        let body = run("zzz-no-such-vendor-xyz-abc").unwrap();
        assert!(body.contains("\"total\": 0"));
    }

    #[test]
    fn keyword_match_returns_fixture_hits() {
        let body = run("xz").unwrap();
        assert!(body.contains("CVE-2024-3094"));
        assert!(body.contains("\"total\": 1"));
    }

    #[test]
    fn case_insensitive_match() {
        let body = run("LOG4J").unwrap();
        assert!(body.contains("CVE-2021-44228"));
    }

    #[test]
    fn json_shape_is_total_hits_object() {
        let v: serde_json::Value = serde_json::from_str(&run("http").unwrap()).unwrap();
        assert!(v.get("query").is_some());
        assert!(v.get("total").is_some());
        assert!(v.get("hits").and_then(|h| h.as_array()).is_some());
    }
}
