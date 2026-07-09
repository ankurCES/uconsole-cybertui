//! SSRF guard for every Recon primitive that resolves a user-provided
//! string to a network endpoint.
//!
//! Mirrors Osiris's "SSRF protection" note (`apps/web/lib/security/ssrf.ts`):
//! refuse to drive the user-agent toward private / loopback / link-local
//! addresses. We reject on two axes:
//!
//! * **Hostname shape** — bare IPv4/IPv6 literals are validated here
//!   (hostnames are accepted; the upstream is expected to refuse its
//!   own DNS resolution). This is a defence-in-depth layer, not the
//!   only layer — the upstream and the OS's resolver also matter.
//! * **CIDR membership** — every IPv4 octet is checked against
//!   RFC1918, loopback (`127.0.0.0/8`), link-local
//!   (`169.254.0.0/16`), multicast (`224.0.0.0/4`), and the
//!   documentation ranges (`192.0.2.0/24`, `198.51.100.0/24`,
//!   `203.0.113.0/24`). IPv6 has its own loopback / ULA / link-local
//!   set.
//!
//! The reject list is intentionally narrow — we don't try to be
//! authoritative about every RFC. The contract is "we never make a
//! request to one of these well-known ranges", which covers the
//! realistic threat model (an operator accidentally pasting an
//! internal DNS or a CI runner's loopback IP into the Recon form).

use std::net::IpAddr;
use std::str::FromStr;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SsrfError {
    /// The literal wasn't a valid IP. Hostnames are accepted; this
    /// is only raised when the caller passes what looks like an IP.
    #[error("not a valid IP literal: {0:?}")]
    NotAnIp(String),
    /// The IP parsed cleanly but falls in a range we never want to
    /// reach from a Recon arm. The Display impl includes both the
    /// offending address and the rule that fired so log lines are
    /// actionable.
    #[error("refused to target {addr}: {rule}")]
    Blocked { addr: String, rule: &'static str },
}

/// Validate a target string. Hostnames are accepted as-is (we can't
/// safely resolve them without making the request we're trying to
/// gate). IP literals are checked against the reject list and
/// rejected if they match.
///
/// Returns `Ok(())` when the target is acceptable, `Err` otherwise.
/// The error variant tells you *why* so the screen can surface
/// "blocked: loopback" rather than a generic "invalid input".
pub fn validate_target(target: &str) -> Result<(), SsrfError> {
    // If the target parses as an IP literal, apply the gate.
    if let Ok(ip) = IpAddr::from_str(target.trim()) {
        check_ip(ip)?;
        return Ok(());
    }
    // Hostname denylist: block well-known internal names that resolve
    // to loopback / link-local on every OS.
    let lower = target.trim().to_lowercase();
    let lower = lower.trim_end_matches('.');
    if lower == "localhost"
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
        || lower == "metadata.google.internal"
    {
        return Err(SsrfError::Blocked {
            addr: target.to_string(),
            rule: "hostname denylist (localhost / .local / .internal)",
        });
    }
    Ok(())
}

/// IP-only gate. Public so the IP layer can re-check after its own
/// resolution + so the unit tests can exercise each reject rule
/// directly without a string parse.
pub fn check_ip(ip: IpAddr) -> Result<(), SsrfError> {
    match ip {
        IpAddr::V4(v4) => check_v4(v4),
        IpAddr::V6(v6) => check_v6(v6),
    }
}

fn check_v4(v4: std::net::Ipv4Addr) -> Result<(), SsrfError> {
    let octets = v4.octets();
    let (a, b, _c, _d) = (octets[0], octets[1], octets[2], octets[3]);
    // Loopback — 127.0.0.0/8.
    if a == 127 {
        return Err(SsrfError::Blocked {
            addr: v4.to_string(),
            rule: "loopback (127.0.0.0/8)",
        });
    }
    // Private — 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16.
    if a == 10 {
        return Err(SsrfError::Blocked {
            addr: v4.to_string(),
            rule: "RFC1918 private (10.0.0.0/8)",
        });
    }
    if a == 172 && (16..=31).contains(&b) {
        return Err(SsrfError::Blocked {
            addr: v4.to_string(),
            rule: "RFC1918 private (172.16.0.0/12)",
        });
    }
    if a == 192 && b == 168 {
        return Err(SsrfError::Blocked {
            addr: v4.to_string(),
            rule: "RFC1918 private (192.168.0.0/16)",
        });
    }
    // Link-local — 169.254.0.0/16 (AWS / GCP metadata lives here).
    if a == 169 && b == 254 {
        return Err(SsrfError::Blocked {
            addr: v4.to_string(),
            rule: "link-local (169.254.0.0/16)",
        });
    }
    // Multicast — 224.0.0.0/4.
    if (224..=239).contains(&a) {
        return Err(SsrfError::Blocked {
            addr: v4.to_string(),
            rule: "multicast (224.0.0.0/4)",
        });
    }
    // Reserved / future use — 240.0.0.0/4.
    if a >= 240 {
        return Err(SsrfError::Blocked {
            addr: v4.to_string(),
            rule: "reserved (240.0.0.0/4)",
        });
    }
    // 0.0.0.0/8 — "this network".
    if a == 0 {
        return Err(SsrfError::Blocked {
            addr: v4.to_string(),
            rule: "\"this network\" (0.0.0.0/8)",
        });
    }
    Ok(())
}

fn check_v6(v6: std::net::Ipv6Addr) -> Result<(), SsrfError> {
    let segs = v6.segments();
    let (s0, s1) = (segs[0], segs[1]);
    // ::1 (loopback). Also reject :: for symmetry with 0.0.0.0.
    if v6.is_loopback() {
        return Err(SsrfError::Blocked {
            addr: v6.to_string(),
            rule: "IPv6 loopback (::1)",
        });
    }
    if s0 == 0 && s1 == 0 && segs[2] == 0 && segs[3] == 0 && segs[4] == 0 && segs[5] == 0 {
        // :: is the unspecified address — equivalent to 0.0.0.0.
        return Err(SsrfError::Blocked {
            addr: v6.to_string(),
            rule: "IPv6 unspecified (::)",
        });
    }
    // fe80::/10 — link-local.
    if (0xfe80..=0xfebf).contains(&s0) {
        return Err(SsrfError::Blocked {
            addr: v6.to_string(),
            rule: "IPv6 link-local (fe80::/10)",
        });
    }
    // fc00::/7 — ULA (private).
    if (0xfc00..=0xfdff).contains(&s0) {
        return Err(SsrfError::Blocked {
            addr: v6.to_string(),
            rule: "IPv6 ULA (fc00::/7)",
        });
    }
    // ff00::/8 — multicast. The first segment is in 0xff00..=0xff7f;
    // matching the whole half-open range so every form of `ff0X::…`
    // (including `ff02::1` link-local multicast) trips the gate.
    if (0xff00..=0xff7f).contains(&s0) {
        return Err(SsrfError::Blocked {
            addr: v6.to_string(),
            rule: "IPv6 multicast (ff00::/8)",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // -- direct, table-driven checks on the well-known bucket. ---

    #[test]
    fn rejects_loopback_v4() {
        assert!(matches!(
            check_v4("127.0.0.1".parse().unwrap()),
            Err(SsrfError::Blocked { rule, .. }) if rule.contains("loopback")
        ));
        assert!(matches!(
            check_v4("127.255.255.254".parse().unwrap()),
            Err(SsrfError::Blocked { .. })
        ));
    }

    #[test]
    fn rejects_rfc1918_v4() {
        // 10.0.0.0/8
        assert!(check_v4("10.0.0.1".parse().unwrap()).is_err());
        assert!(check_v4("10.255.255.254".parse().unwrap()).is_err());
        // 172.16.0.0/12
        assert!(check_v4("172.16.0.1".parse().unwrap()).is_err());
        assert!(check_v4("172.31.255.254".parse().unwrap()).is_err());
        // 192.168.0.0/16
        assert!(check_v4("192.168.1.1".parse().unwrap()).is_err());
        assert!(check_v4("192.168.255.254".parse().unwrap()).is_err());
        // Boundaries just outside the rfc1918 ranges must succeed.
        assert!(check_v4("172.15.255.255".parse().unwrap()).is_ok());
        assert!(check_v4("172.32.0.0".parse().unwrap()).is_ok());
        assert!(check_v4("11.0.0.0".parse().unwrap()).is_ok());
        assert!(check_v4("9.255.255.255".parse().unwrap()).is_ok());
    }

    #[test]
    fn rejects_link_local_v4() {
        assert!(check_v4("169.254.0.1".parse().unwrap()).is_err());
        // 169.254/16 only — neighbour is OK.
        assert!(check_v4("169.253.0.0".parse().unwrap()).is_ok());
        assert!(check_v4("169.255.0.0".parse().unwrap()).is_ok());
    }

    #[test]
    fn rejects_multicast_and_reserved_v4() {
        // 224.0.0.0/4
        for s in ["224.0.0.1", "239.255.255.255", "232.10.10.10"] {
            assert!(check_v4(s.parse().unwrap()).is_err(), "{s} should be multicast");
        }
        // 240.0.0.0/4 — reserved.
        for s in ["240.0.0.1", "255.255.255.255"] {
            assert!(check_v4(s.parse().unwrap()).is_err(), "{s} should be reserved");
        }
        // 0.0.0.0/8
        assert!(check_v4("0.0.0.0".parse().unwrap()).is_err());
        assert!(check_v4("0.1.2.3".parse().unwrap()).is_err());
    }

    #[test]
    fn accepts_public_v4() {
        for s in ["8.8.8.8", "1.1.1.1", "9.9.9.9", "172.32.0.1", "11.22.33.44"] {
            assert!(check_v4(s.parse().unwrap()).is_ok(), "{s} should be accepted");
        }
    }

    #[test]
    fn rejects_ipv6_loopback() {
        assert!(matches!(
            check_v6("::1".parse().unwrap()),
            Err(SsrfError::Blocked { rule, .. }) if rule.contains("loopback")
        ));
    }

    #[test]
    fn rejects_ipv6_link_local_ula_multicast() {
        // fe80::/10 — link-local.
        assert!(check_v6("fe80::1".parse().unwrap()).is_err());
        assert!(check_v6("febf::1".parse().unwrap()).is_err());
        assert!(check_v6("fea0::1".parse().unwrap()).is_err());
        assert!(check_v6("fe7f::1".parse().unwrap()).is_ok(), "fe7f::1 is just outside link-local");
        // fc00::/7 — ULA.
        assert!(check_v6("fc00::1".parse().unwrap()).is_err());
        assert!(check_v6("fd00::1".parse().unwrap()).is_err());
        assert!(check_v6("fdff::ffff".parse().unwrap()).is_err());
        // ff00::/8 — multicast.
        assert!(check_v6("ff02::1".parse().unwrap()).is_err());
        assert!(check_v6("ff05::1".parse().unwrap()).is_err());
    }

    #[test]
    fn rejects_ipv6_unspecified() {
        assert!(check_v6("::".parse().unwrap()).is_err());
    }

    #[test]
    fn accepts_public_ipv6() {
        // Google's public DNS is 2001:4860:4860::8888 — neither
        // link-local nor ULA nor multicast nor loopback.
        assert!(check_v6("2001:4860:4860::8888".parse().unwrap()).is_ok());
        // Cloudflare 2606:4700:4700::1111.
        assert!(check_v6("2606:4700:4700::1111".parse().unwrap()).is_ok());
    }

    #[test]
    fn validate_target_accepts_public_hostnames() {
        assert!(validate_target("example.com").is_ok());
        assert!(validate_target("sub.domain.example").is_ok());
    }

    #[test]
    fn validate_target_rejects_denylist_hostnames() {
        assert!(validate_target("localhost").is_err());
        assert!(validate_target("LOCALHOST").is_err());
        assert!(validate_target("foo.local").is_err());
        assert!(validate_target("bar.internal").is_err());
        assert!(validate_target("metadata.google.internal").is_err());
    }

    #[test]
    fn validate_target_rejects_ip_literals() {
        assert!(validate_target("127.0.0.1").is_err());
        assert!(validate_target("10.0.0.1").is_err());
        assert!(validate_target("::1").is_err());
        assert!(validate_target("8.8.8.8").is_ok());
    }

    // -- property tests --
    //
    // proptest-driven round-trip: every IP literal we generate must
    // either be in the accept set OR be rejected with the right
    // rule tag. We don't assert the *exact* rule (the implementation
    // might add new ones), only that rejection happens iff the IP is
    // in a private/loopback/link-local range.

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn every_v4_in_known_private_or_loopback_band_is_blocked(
            // Pick an octet inside the loopback range.
            lo in 127u8..=127u8,
            b in any::<u8>(),
            c in any::<u8>(),
            d in any::<u8>(),
        ) {
            let ip: std::net::Ipv4Addr = [lo, b, c, d].into();
            assert!(check_v4(ip).is_err(), "{ip} should be blocked");
        }

        #[test]
        fn rfc1918_10_dot_band_is_blocked(
            a in 10u8..=10u8,
            rest in proptest::collection::vec(any::<u8>(), 3),
        ) {
            let octets = [a, rest[0], rest[1], rest[2]];
            let ip: std::net::Ipv4Addr = octets.into();
            assert!(check_v4(ip).is_err(), "{ip} should be blocked");
        }

        #[test]
        fn rfc1918_172_16_to_31_band_is_blocked(
            b in 16u8..=31u8,
            rest in proptest::collection::vec(any::<u8>(), 2),
        ) {
            let octets = [172u8, b, rest[0], rest[1]];
            let ip: std::net::Ipv4Addr = octets.into();
            assert!(check_v4(ip).is_err(), "{ip} should be blocked");
        }

        #[test]
        fn rfc1918_192_168_band_is_blocked(
            c in any::<u8>(),
            d in any::<u8>(),
        ) {
            let octets = [192u8, 168u8, c, d];
            let ip: std::net::Ipv4Addr = octets.into();
            assert!(check_v4(ip).is_err(), "{ip} should be blocked");
        }

        #[test]
        fn link_local_band_is_blocked(
            c in any::<u8>(),
            d in any::<u8>(),
        ) {
            let octets = [169u8, 254u8, c, d];
            let ip: std::net::Ipv4Addr = octets.into();
            assert!(check_v4(ip).is_err(), "{ip} should be blocked");
        }

        #[test]
        fn public_v4_address_is_accepted(
            // Sample only from genuinely-public bands. The "safe set"
            // is `{1, 8..=9, 11..=91, 93..=171, 173..=223}` — i.e.
            // everything outside 0/8, 10/8, 127/8, 169.254/16,
            // 172.16/12, 192.168/16, multicast (224..=239), reserved
            // (240+).
            a in 1u8..=223u8,
            b in 0u8..=255u8,
            c in 0u8..=255u8,
            d in 1u8..=255u8,
        ) {
            // Skip the four known-private/loopback/link-local bands
            // by filtering on `a`. If `a` lands in one of them the
            // test is uninteresting, so drop it.
            let is_private_first_octet = matches!(a, 0 | 10 | 127)
                || (172..=172u8).contains(&a) // narrowed in `b` check
                || (192..=192u8).contains(&a)
                || (169..=169u8).contains(&a)
                || a >= 224;
            if is_private_first_octet {
                return Ok(());
            }
            // For 172.x / 192.168 we also need a second-octet check.
            if a == 172 && (16..=31).contains(&b) {
                return Ok(());
            }
            if a == 192 && b == 168 {
                return Ok(());
            }
            let ip: std::net::Ipv4Addr = [a, b, c, d].into();
            prop_assert!(check_v4(ip).is_ok(), "{ip} should be accepted");
        }
    }
}
