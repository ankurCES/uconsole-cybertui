//! Sanctions primitive — local OFAC SDN mirror.
//!
//! We ship a tiny SDNX-like CSV at `testdata/sanctions_sample.csv`
//! covering the two fixture rows M7 needs:
//!
//!   * `Test Sanctioned Entity` — the canonical "this trips the
//!     sentinel" entry.
//!   * `Clean Reference Inc.`  — present so substring search has at
//!     least one non-sanctioned row.
//!
//! `query` is a substring match against either `name` (primary) or
//! `remarks`. The screen renders the matched rows in the tinted
//! area; the footer chip flips to "yellow" only when at least one
//! row's `sdn_type` is `SDN`.
//!
//! Future PR: replace this `include_str!`-backed table with a one-shot
//! download from the Treasury SDN list, parsed once at startup. The
//! public surface (`pub fn run(query: &str) -> Result<String>`)
//! stays unchanged so the screen + CLI don't care.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::json;

pub const LABEL: &str = "SANCTIONS";

#[derive(Debug, Deserialize)]
pub struct Row {
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub sdn_type: String,
    #[serde(default)]
    pub program: String,
    #[serde(default)]
    pub remarks: String,
    #[serde(default)]
    pub aka: String,
}

fn load_table() -> Vec<Row> {
    // `include_str!` bakes the file at compile-time; if a future PR
    // deletes the fixture the build will fail rather than silently
    // shipping an empty list.
    const CSV: &str = include_str!("../../testdata/sanctions_sample.csv");
    csv::Reader::from_reader(CSV.as_bytes())
        .deserialize::<Row>()
        .flatten()
        .collect()
}

pub fn run(query: &str) -> Result<String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Err(anyhow!("empty query"));
    }
    let table = load_table();
    let hits: Vec<&Row> = table
        .iter()
        .filter(|r| {
            q.is_empty()
                || r.name.to_lowercase().contains(&q)
                || r.remarks.to_lowercase().contains(&q)
                || r.aka.to_lowercase().contains(&q)
        })
        .collect();
    let has_sdn = hits.iter().any(|r| r.sdn_type.eq_ignore_ascii_case("SDN"));
    let body = json!({
        "query": query.trim(),
        "total": hits.len(),
        "warrants_sentinel": has_sdn,
        "hits": hits.iter().map(|r| json!({
            "uid": r.uid,
            "name": r.name,
            "sdn_type": r.sdn_type,
            "program": r.program,
            "remarks": r.remarks,
            "aka": r.aka,
        })).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&body).context("serialising sanctions result")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_is_rejected() {
        assert!(run("").is_err());
    }

    #[test]
    fn bundled_table_includes_both_fixture_rows() {
        let t = load_table();
        assert!(t.iter().any(|r| r.name.contains("Test Sanctioned Entity")));
        assert!(t.iter().any(|r| r.name.contains("Clean Reference Inc.")));
    }

    #[test]
    fn sanctioned_substring_flags_sentinel() {
        let body = run("sanctioned").unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["warrants_sentinel"], true);
        assert!(v["total"].as_u64().unwrap() > 0);
    }

    #[test]
    fn clean_substring_does_not_flag_sentinel() {
        let body = run("clean").unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["warrants_sentinel"], false);
    }

    #[test]
    fn unknown_query_returns_zero_hits() {
        let body = run("zzz-no-such-entity-xyz-abc").unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["total"].as_u64(), Some(0));
        assert_eq!(v["warrants_sentinel"], false);
    }
}
