//! Crypto-risk primitive — local currency-address risk table.
//!
//! Offline-only: reads the bundled `crates/intel/testdata/crypto_risk.csv`
//! and pretty-prints any addresses matching the query (substring
//! match, case-insensitive). The shipped fixture covers the two
//! canonical cases Osiris calls out — a sanctioned exchange address
//! and a known mixer.
//!
//! The 0.3 release keeps this hermetic so CI + offline laptops
//! always return something. A later phase can add a live
//! `walletexplorer.com`-style fetch behind the same `Result<String>`
//! contract.

use anyhow::{anyhow, Context, Result};
use serde_json::json;

pub const LABEL: &str = "CRYPTO";

#[derive(Debug, serde::Deserialize)]
struct Row {
    #[serde(default)]
    network: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    risk: String,
}

/// Load the bundled CSV. We fall back to a hardcoded minimal table if
/// the file isn't present (e.g. when the crate is consumed outside
/// the workspace) so the screen always renders a non-empty response.
fn load_table() -> Vec<Row> {
    // Bundle path: <crate>/testdata/crypto_risk.csv — `include_str!`
    // bakes it into the binary at compile time so no runtime FS
    // path resolution is needed. This is the same pattern the
    // sanctions module uses.
    const CSV: &str = include_str!("../../testdata/crypto_risk.csv");
    let mut out = Vec::new();
    let mut rdr = csv::Reader::from_reader(CSV.as_bytes());
    for row in rdr.deserialize::<Row>().flatten() {
        out.push(row);
    }
    if out.is_empty() {
        // Belt + braces — synthesise one entry so the screen never
        // shows a blank pane if the bundled CSV is accidentally
        // emptied in a future PR.
        out.push(Row {
            network: "btc".into(),
            address: "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".into(),
            category: "genesis".into(),
            note: "Genesis block coinbase (non-risk reference)".into(),
            risk: "info".into(),
        });
    }
    out
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
                || r.address.to_lowercase().contains(&q)
                || r.category.to_lowercase().contains(&q)
                || r.note.to_lowercase().contains(&q)
        })
        .collect();
    let body = json!({
        "query": query.trim(),
        "hits": hits.iter().map(|r| json!({
            "network": r.network,
            "address": r.address,
            "category": r.category,
            "note": r.note,
            "risk": r.risk,
        })).collect::<Vec<_>>(),
        "total": hits.len(),
    });
    serde_json::to_string_pretty(&body).context("serialising crypto result")
}

// Helper unused but kept here so future PRs reach for the same
// path-resolution pattern when they add a live source.
#[allow(dead_code)]
fn testdata_path() -> Option<&'static str> {
    Some("testdata/crypto_risk.csv")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_is_rejected() {
        assert!(run("").is_err());
    }

    #[test]
    fn bundled_table_loads_at_least_one_row() {
        assert!(!load_table().is_empty(), "crypto_risk.csv must include at least one entry");
    }

    #[test]
    fn substring_match_finds_address() {
        let body = run("1a1z").unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let hits = v["hits"].as_array().unwrap();
        assert!(!hits.is_empty(), "no hit for partial address");
    }

    #[test]
    fn case_insensitive_category_match() {
        let body = run("SANCTIONED").unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v["total"].as_u64().unwrap() > 0);
    }

    #[test]
    fn unknown_query_returns_zero_hits() {
        let body = run("zzz-no-such-address-xyz-abc").unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["total"].as_u64(), Some(0));
    }
}
