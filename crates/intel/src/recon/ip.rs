//! `ip-api.com` lookup primitive — geolocation + ASN + reverse DNS.
//!
//! Sends `http://ip-api.com/json/<query>?fields=66846719` (the
//! "everything" field set) and pretty-prints the resulting object.
//! We deliberately send to the public, no-key endpoint — matches how
//! the TUI's `city/geo` layer already works in M5.
//!
//! Every request runs through [`super::ssrf::validate_target`] so a
//! pasted `10.0.0.1` fails fast at the boundary, before reaching the
//! network. Hostnames are forwarded to ip-api's resolver.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::json;

use super::ssrf::check_ip;

pub const LABEL: &str = "IP";

/// Tight subset of ip-api's response. Anything else gets dropped.
#[derive(Debug, Deserialize)]
struct IpApi {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    #[serde(rename = "countryCode")]
    country_code: Option<String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    #[serde(rename = "regionName")]
    region_name: Option<String>,
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    zip: Option<String>,
    #[serde(default)]
    lat: Option<f64>,
    #[serde(default)]
    lon: Option<f64>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    isp: Option<String>,
    #[serde(default)]
    org: Option<String>,
    #[serde(default)]
    #[serde(rename = "as")]
    asn: Option<String>,
}

pub fn run(query: &str) -> Result<String> {
    let q = query.trim();
    if q.is_empty() {
        return Err(anyhow!("empty query"));
    }
    // If the query is an IP literal, gate it. Hostnames are
    // forwarded as-is — ip-api does its own DNS.
    if let Ok(ip) = q.parse::<std::net::IpAddr>() {
        check_ip(ip)?;
    }

    let url = format!("http://ip-api.com/json/{q}?fields=66846719");
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|e| anyhow!("ip-api request failed: {e}"))?;
    // ureq 2.x doesn't expose `into_json` — pipe the body reader
    // through serde_json directly so we still get a typed struct.
    let body: IpApi = serde_json::from_reader(resp.into_reader())
        .map_err(|e| anyhow!("ip-api json decode: {e}"))?;
    if matches!(body.status.as_deref(), Some("fail")) {
        return Err(anyhow!("ip-api reported failure for {q}"));
    }
    // Build the human-readable summary the screen / CLI print.
    let summary = json!({
        "query":  body.query,
        "country": body.country,
        "country_code": body.country_code,
        "region": body.region_name.or(body.region),
        "city": body.city,
        "zip": body.zip,
        "lat": body.lat,
        "lon": body.lon,
        "timezone": body.timezone,
        "isp": body.isp,
        "org": body.org,
        "as": body.asn,
    });
    serde_json::to_string_pretty(&summary).context("serialising ip-api result")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recon::ssrf::SsrfError;

    #[test]
    fn empty_query_is_rejected() {
        assert!(run("").is_err());
    }

    /// Even though `run()` requires network, the SSRF gate fires
    /// before the network call and must surface its structured error
    /// deterministically.
    #[test]
    fn rejects_loopback_before_network() {
        let err = run("127.0.0.1").unwrap_err();
        let s = err.to_string();
        assert!(s.contains("refused to target"), "got: {s}");
        // Downcast the chain to confirm the variant.
        let mut src: &dyn std::error::Error = err.as_ref();
        loop {
            if let Some(ssrf) = src.downcast_ref::<SsrfError>() {
                assert!(matches!(ssrf, SsrfError::Blocked { .. }));
                return;
            }
            match src.source() {
                Some(next) => src = next,
                None => panic!("expected SsrfError somewhere in the chain"),
            }
        }
    }

    #[test]
    fn rejects_rfc1918_before_network() {
        // `run()` never reaches the HTTP client when the gate fires,
        // so this test is hermetic — no real network.
        assert!(run("10.0.0.1").is_err());
        assert!(run("192.168.1.1").is_err());
        assert!(run("172.16.5.5").is_err());
    }

    #[test]
    fn accepts_public_ipv4_string() {
        // Public IPs pass the gate; this asserts we never get a
        // SsrfError. The HTTP call may fail (no network in CI) — we
        // ignore that branch here.
        let res = run("8.8.8.8");
        if let Err(e) = res {
            // Any error must NOT be a SsrfError — ip-api's own errors
            // are fine, gate errors are not.
            let mut src: &dyn std::error::Error = e.as_ref();
            loop {
                if src.downcast_ref::<SsrfError>().is_some() {
                    panic!("public IP must not be SSRF-gated: {e}");
                }
                match src.source() {
                    Some(next) => src = next,
                    None => break,
                }
            }
        }
    }

    #[test]
    fn accepts_hostname_input() {
        // Hostnames bypass the SSRF gate; ip-api does its own DNS.
        // Same error-shape contract: not a SsrfError.
        let res = run("dns.google");
        if let Err(e) = res {
            let mut src: &dyn std::error::Error = e.as_ref();
            loop {
                if src.downcast_ref::<SsrfError>().is_some() {
                    panic!("hostname must not be SSRF-gated: {e}");
                }
                match src.source() {
                    Some(next) => src = next,
                    None => break,
                }
            }
        }
    }
}
