//! Recon action console — seven OSINT primitives behind a single
//! stable contract `pub fn run(query: &str) -> anyhow::Result<String>`.
//!
//! Every primitive is intentionally small: it's a shell-out (dig,
//! whois, openssl) or a single HTTP GET (ip-api, NVD) wrapped in a
//! uniform `Result<String>` so the Recon screen + `cyberdeck recon`
//! verb can drive them without branching on endpoint shape. The
//! `ssrf` submodule guards every primitive that talks to a user-
//! supplied target so a copy-pasted 10.0.0.1 doesn't reach a router.
//!
//! Each submodule owns:
//! * a `pub fn run(query: &str) -> anyhow::Result<String>` entry
//! * a `pub const LABEL: &str` (used by the Recon screen + CLI
//!   tab strip so they never drift)
//! * an explicit, tested failure mode for "binary not installed"
//!   (so the screen surfaces a useful error, not a panic)
//!
//! Modules:
//!   * [`dns`]       — `dig +short`
//!   * [`whois`]     — `whois`
//!   * [`ip`]        — ip-api.com lookup
//!   * [`ssl`]       — `openssl s_client`
//!   * [`cve`]       — NVD search
//!   * [`crypto`]    — local currency-mixer risk table
//!   * [`sanctions`] — local OFAC SDN CSV (parse-only)
//!   * [`ssrf`]      — defence-in-depth target gate

pub mod crypto;
pub mod cve;
pub mod dns;
pub mod ip;
pub mod sanctions;
pub mod ssrf;
pub mod ssl;
pub mod whois;

/// Identifier for one of the seven Recon primitives. Mirrors the
/// in-screen tab strip order so a script can drive the CLI by
/// ordinal the same way the screen does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ReconTab {
    Dns = 0,
    Whois = 1,
    Ip = 2,
    Ssl = 3,
    Cve = 4,
    Crypto = 5,
    Sanctions = 6,
}

impl ReconTab {
    /// Canonical draw order — appended-to-screen ordered list.
    pub const ALL: &'static [ReconTab] = &[
        ReconTab::Dns,
        ReconTab::Whois,
        ReconTab::Ip,
        ReconTab::Ssl,
        ReconTab::Cve,
        ReconTab::Crypto,
        ReconTab::Sanctions,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            ReconTab::Dns => "DNS",
            ReconTab::Whois => "WHOIS",
            ReconTab::Ip => "IP",
            ReconTab::Ssl => "SSL",
            ReconTab::Cve => "CVE",
            ReconTab::Crypto => "CRYPTO",
            ReconTab::Sanctions => "SANCTIONS",
        }
    }

    /// Short single-character glyph for the screen tab chip.
    pub const fn glyph(self) -> &'static str {
        match self {
            ReconTab::Dns => "D",
            ReconTab::Whois => "W",
            ReconTab::Ip => "I",
            ReconTab::Ssl => "S",
            ReconTab::Cve => "C",
            ReconTab::Crypto => "₿",
            ReconTab::Sanctions => "⚖",
        }
    }

    /// Sentinel colour for the footer chip. Sanctions is permanently
    /// "red-on-yellow" because "SANCTIONED" is the worst kind of hit.
    pub const fn is_sensitive(self) -> bool {
        matches!(self, ReconTab::Sanctions | ReconTab::Cve)
    }

    /// Cycle forward / back, wrapping at the ends.
    pub fn cycle(self, forward: bool) -> Self {
        let n = Self::ALL.len() as i32;
        let i = self as i32;
        let next = if forward { (i + 1) % n } else { (i - 1 + n) % n };
        Self::ALL[next as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_cycle_wraps_at_both_ends() {
        assert_eq!(ReconTab::Dns.cycle(true), ReconTab::Whois);
        assert_eq!(ReconTab::Dns.cycle(false), ReconTab::Sanctions);
        assert_eq!(ReconTab::Sanctions.cycle(true), ReconTab::Dns);
        assert_eq!(ReconTab::Sanctions.cycle(false), ReconTab::Crypto);
    }

    #[test]
    fn all_seven_tabs_have_unique_labels() {
        let labels: std::collections::BTreeSet<_> =
            ReconTab::ALL.iter().map(|t| t.label()).collect();
        assert_eq!(labels.len(), ReconTab::ALL.len());
    }

    #[test]
    fn sensitive_set_includes_cve_and_sanctions() {
        assert!(ReconTab::Cve.is_sensitive());
        assert!(ReconTab::Sanctions.is_sensitive());
        assert!(!ReconTab::Dns.is_sensitive());
        assert!(!ReconTab::Ip.is_sensitive());
    }
}
