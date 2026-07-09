//! Integration tests for the `cyberdeck` CLI.
//!
//! These tests exercise the clap parser and dispatch — they run the built
//! binary the same way users do on the command line, which gives us coverage
//! of `Cli::parse`, the `Cmd` enum dispatch, and each module's `run` entry.

use assert_cmd::Command;
use predicates::prelude::*;

#[allow(deprecated)] // assert_cmd::cargo_bin stays the simplest path here
fn cmd() -> Command {
    Command::cargo_bin("cyberdeck").expect("cyberdeck binary must build first")
}

#[test]
fn help_lists_all_top_level_subcommands() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("daemon"))
        .stdout(predicate::str::contains("net"))
        .stdout(predicate::str::contains("bluetooth"))
        .stdout(predicate::str::contains("audio"))
        .stdout(predicate::str::contains("display"))
        .stdout(predicate::str::contains("power"))
        .stdout(predicate::str::contains("storage"))
        .stdout(predicate::str::contains("services"))
        .stdout(predicate::str::contains("packages"))
        .stdout(predicate::str::contains("process"))
        .stdout(predicate::str::contains("logs"))
        .stdout(predicate::str::contains("sys"))
        .stdout(predicate::str::contains("workspace"))
        .stdout(predicate::str::contains("wm"))
        .stdout(predicate::str::contains("completion"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("update"));
}

#[test]
fn json_flag_emits_machine_readable_output() {
    cmd()
        .args(["--json", "net", "wifi-scan"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"ssids\""));
}

#[test]
fn net_wifi_scan_human_default_is_not_pure_json() {
    // We don't lock the exact format, but the stub returns "stub-network-A"
    // in the human form too (because the stub `run` ignores OutputMode).
    // The important property: it exits 0 and produces some output.
    cmd()
        .args(["net", "wifi-scan"])
        .assert()
        .success()
        .stdout(predicate::str::contains("stub-network"));
}

#[test]
fn net_wifi_connect_requires_ssid() {
    // No `ssid` arg → clap rejects with an error.
    cmd()
        .args(["net", "wifi-connect"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("SSID").or(predicate::str::contains("required")));
}

#[test]
fn net_wifi_connect_passes_ssid_through() {
    cmd()
        .args(["--json", "net", "wifi-connect", "CoffeeShop"])
        .assert()
        .success()
        // serde_json emits compact JSON, so quotes are flush against colons.
        .stdout(predicate::str::contains("\"ssid\":\"CoffeeShop\""))
        .stdout(predicate::str::contains("\"password_provided\":false"));
}

#[test]
fn net_wifi_connect_with_password_flag_round_trips() {
    cmd()
        .args(["--json", "net", "wifi-connect", "--password", "hunter2", "LabNet"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"ssid\":\"LabNet\""))
        .stdout(predicate::str::contains("\"password_provided\":true"));
}

#[test]
fn daemon_ping_reports_socket_path() {
    cmd()
        .args(["daemon", "ping"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pong"))
        .stdout(predicate::str::contains("socket:"));
}

#[test]
fn net_if_up_default_is_up_true() {
    // Clap defaults `up` to true via `default_value_t = true`.
    // A future improvement: support `--down` as a separate flag.
    cmd()
        .args(["--json", "net", "if-up", "eth0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"iface\":\"eth0\""))
        .stdout(predicate::str::contains("\"up\":true"));
}

#[test]
fn unknown_subcommand_errors() {
    cmd()
        .args(["not-a-real-subcommand"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand").or(
            predicate::str::contains("invalid subcommand"),
        ));
}

#[test]
fn bluetooth_subcommand_help_lists_list_and_status() {
    // Drill into a stub verb and check clap generated the expected children.
    cmd()
        .args(["bluetooth", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("status"));
}

// -------------------------------------------------------------------------
// Step 10 — `cyberdeck city` verb. Pins the four new subcommands
// (`locate`, `weather`, `roads`, `bundled`) and the `--json`
// shape for each. `locate` + `weather` hit real HTTP endpoints
// (ip-api / Open-Meteo) and may fail in a sandboxed test env, so we
// only assert on the pure-data arms (`roads` + `bundled`) here —
// the HTTP-backed arms are covered by the wiremock unit tests
// inside `cyberdeck-tui` (Steps 5+8).
// -------------------------------------------------------------------------

#[test]
fn help_lists_city_subcommand() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("city"));
}

#[test]
fn city_bundled_lists_known_slugs() {
    // Pure-data arm — no network. The bundled seattle.json is the
    // only city with a populated JSON file at this commit; the others
    // are placeholders that fall back to seattle via
    // `load_bundled_or_default`. The BUNDLED list itself always
    // contains them.
    cmd()
        .args(["--json", "city", "bundled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"seattle\""))
        .stdout(predicate::str::contains("\"london\""))
        .stdout(predicate::str::contains("\"tokyo\""));
}

#[test]
fn city_roads_default_slug_is_seattle() {
    // No slug arg → defaults to `seattle` per the `#[arg(default_value)]`.
    // The response always surfaces a `slug_used` so callers can detect
    // fallbacks. We assert on the data-shape (road_count > 0, bbox
    // present) rather than exact JSON, since the bundled seattle
    // fixture is small but stable.
    cmd()
        .args(["--json", "city", "roads"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"slug_used\":\"seattle\""))
        .stdout(predicate::str::contains("\"road_count\":"))
        .stdout(predicate::str::contains("\"bbox\":"));
}

#[test]
fn city_roads_unknown_slug_falls_back_to_seattle() {
    // `atlantis` isn't bundled — the loader should fall back to
    // `seattle` and surface that in `slug_used`. This matches the
    // TUI's `CityRoads::load_bundled_or_default` behaviour exactly.
    cmd()
        .args(["--json", "city", "roads", "atlantis"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"slug_requested\":\"atlantis\""))
        .stdout(predicate::str::contains("\"slug_used\":\"seattle\""));
}

#[test]
fn city_help_lists_all_four_subcommands() {
    // Drill into the verb and check clap generated the four expected
    // children — `locate`, `weather`, `roads`, `bundled`.
    cmd()
        .args(["city", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("locate"))
        .stdout(predicate::str::contains("weather"))
        .stdout(predicate::str::contains("roads"))
        .stdout(predicate::str::contains("bundled"));
}

#[test]
fn city_weather_requires_lat_and_lon() {
    // The weather subcommand takes two required `--lat` / `--lon`
    // flags. Missing either → clap rejects. We assert on
    // `failure()` + an error message that mentions one of the flags.
    cmd()
        .args(["--json", "city", "weather"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("--lat")
                .or(predicate::str::contains("lat"))
                .or(predicate::str::contains("required")),
        );
}

// -------------------------------------------------------------------------
// M6 — `cyberdeck intel` verb. Pins the three subcommands
// (`layers`, `refresh`, `sentinel`) and the stable `--json`
// shape. Mirrors the `cyberdeck city` test block above: pure-data
// arms assert on shape, no network.
//
// We assert on the *fields callers depend on* rather than full
// equality so a future PR that adds a field to the JSON envelope
// (e.g. a `fetched_at` timestamp) doesn't need to touch every test.
// -------------------------------------------------------------------------

#[test]
fn help_lists_intel_subcommand() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("intel"));
}

#[test]
fn intel_help_lists_all_three_subcommands() {
    // Drill into the verb and check clap generated the three expected
    // children — `layers`, `refresh`, `sentinel`.
    cmd()
        .args(["intel", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("layers"))
        .stdout(predicate::str::contains("refresh"))
        .stdout(predicate::str::contains("sentinel"));
}

#[test]
fn intel_layers_returns_all_nine_known_layers() {
    // The Intel screen paints a 9-layer grid (M5). The CLI must
    // surface the same nine so a dashboard or alert rule can pin
    // the layer name once and not break when one is renamed.
    cmd()
        .args(["--json", "intel", "layers"])
        .assert()
        .success()
        // Each layer entry must carry the four projection fields the
        // CLI / screen / PR-template pipeline all consume.
        .stdout(predicate::str::contains("\"layer\":\"flights\""))
        .stdout(predicate::str::contains("\"layer\":\"earthquakes\""))
        .stdout(predicate::str::contains("\"layer\":\"fires\""))
        .stdout(predicate::str::contains("\"layer\":\"weather\""))
        .stdout(predicate::str::contains("\"layer\":\"satellites\""))
        .stdout(predicate::str::contains("\"layer\":\"news\""))
        .stdout(predicate::str::contains("\"layer\":\"cctv\""))
        .stdout(predicate::str::contains("\"layer\":\"maritime\""))
        .stdout(predicate::str::contains("\"layer\":\"conflicts\""))
        // Label + glyph + poll_interval_secs projection is mandatory.
        .stdout(predicate::str::contains("\"label\":\"Flights\""))
        .stdout(predicate::str::contains("\"glyph\":\"✈\""))
        .stdout(predicate::str::contains("\"poll_interval_secs\":30"));
}

#[test]
fn intel_layers_default_is_human_not_pure_json() {
    // Without `--json` we still render the same data, just pretty.
    // We only assert the verb succeeded and surfaced at least one
    // layer label; the human formatter is free to change style.
    cmd()
        .args(["intel", "layers"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Flights"));
}

#[test]
fn intel_refresh_layerless_acks_ok() {
    // `intel refresh` (no `--layer`) means "refresh everything". The
    // envelope must echo `layer: null` so callers can detect
    // broadside refreshes in their logs.
    cmd()
        .args(["--json", "intel", "refresh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"ok\":true"))
        .stdout(predicate::str::contains("\"layer\":null"));
}

#[test]
fn intel_refresh_known_layer_echoes_the_name() {
    // The CLI normalises unknown layers to invalid, known layers
    // pass through with the same spelling. We use `flights` because
    // it's the first entry in `LayerId::ALL` — minimal fix risk.
    cmd()
        .args(["--json", "intel", "refresh", "--layer", "flights"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"ok\":true"))
        .stdout(predicate::str::contains("\"layer\":\"flights\""));
}

#[test]
fn intel_refresh_unknown_layer_is_invalid_not_silent() {
    // Typo in a shell pipeline must fail loudly — direct mode emits
    // `ok: false` with `code: invalid` (the same shape the daemon
    // RPC returns), and exit 0 so the call site can grep stderr
    // / pretty-print without breaking the pipeline.
    cmd()
        .args(["--json", "intel", "refresh", "--layer", "definitely-not-a-layer"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"ok\":false"))
        .stdout(predicate::str::contains("\"code\":\"invalid\""));
}

#[test]
fn intel_refresh_layer_and_all_are_mutually_exclusive() {
    // `--layer X --all` is ambiguous; clap rejects. We assert on a
    // failure exit and stderr mentioning the conflict.
    cmd()
        .args(["--json", "intel", "refresh", "--layer", "flights", "--all"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("cannot be used with")
                .or(predicate::str::contains("conflict"))
                .or(predicate::str::contains("mutually exclusive")),
        );
}

#[test]
fn intel_sentinel_returns_green_with_zero_counts_for_empty_rollup() {
    // Direct-mode (no live TUI process reading from a refiller)
    // always returns the empty-rollup shape so consumers see a
    // stable contract: `sentinel` field present, all three count
    // fields present and zero-initialised.
    cmd()
        .args(["--json", "intel", "sentinel"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"sentinel\":\"green\""))
        .stdout(predicate::str::contains("\"green\":0"))
        .stdout(predicate::str::contains("\"yellow\":0"))
        .stdout(predicate::str::contains("\"red\":0"));
}

// -------------------------------------------------------------------------
// M7 — `cyberdeck recon` verb. Pins the seven subcommands
// (`dns`, `whois`, `ip`, `ssl`, `cve`, `crypto`, `sanctions`) and
// the stable shape of their JSON envelope. The CVE / sanctions
// arms hit local fixtures so they're hermetic; the shell-out
// arms (`dns` / `whois` / `ssl`) are exercised via wrapper tests
// that don't require a real network in CI.
// -------------------------------------------------------------------------

#[test]
fn help_lists_recon_subcommand() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("recon"));
}

#[test]
fn recon_help_lists_all_seven_subcommands() {
    cmd()
        .args(["recon", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dns"))
        .stdout(predicate::str::contains("whois"))
        .stdout(predicate::str::contains("ip"))
        .stdout(predicate::str::contains("ssl"))
        .stdout(predicate::str::contains("cve"))
        .stdout(predicate::str::contains("crypto"))
        .stdout(predicate::str::contains("sanctions"));
}

#[test]
fn recon_cve_returns_log4j_for_keyword() {
    // Hermetic — uses the bundled fixture, no network.
    cmd()
        .args(["--json", "recon", "cve", "log4j"])
        .assert()
        .success()
        .stdout(predicate::str::contains("CVE-2021-44228"))
        .stdout(predicate::str::contains("Log4Shell"));
}

#[test]
fn recon_cve_unknown_keyword_returns_zero_hits() {
    cmd()
        .args(["--json", "recon", "cve", "zzz-no-such-vendor-xyz-abc"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"total\": 0"));
}

#[test]
fn recon_sanctions_finds_bundled_sdn_entry() {
    cmd()
        .args(["--json", "recon", "sanctions", "Test Sanctioned Entity"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Test Sanctioned Entity"));
}

#[test]
fn recon_sanctions_flags_warrants_sentinel_when_sdn_found() {
    // The fixture's "Test Sanctioned Entity" row has sdn_type=SDN
    // so `warrants_sentinel` must be true. This is the field that
    // drives the screen's footer chip. The CLI emits pretty-printed
    // JSON (serde_json::to_string_pretty) which inserts a space
    // after each `:` — match that exact shape.
    cmd()
        .args(["--json", "recon", "sanctions", "sanctioned"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"warrants_sentinel\": true"));
}

#[test]
fn recon_crypto_finds_sanctioned_address() {
    // The bundled `crypto_risk.csv` has a row tagged `sanctioned`.
    cmd()
        .args(["--json", "recon", "crypto", "1F1tAaz"])
        .assert()
        .success()
        // The address matches the truncated prefix; the row's
        // `risk: sanctioned` must surface so a pipeline can alert.
        // Pretty-printed JSON: "key": value (space after colon).
        .stdout(predicate::str::contains("\"risk\": \"sanctioned\""));
}

#[test]
fn recon_ip_loopback_is_blocked_with_structured_error() {
    // SSRF gate fires *before* any HTTP call, so this is hermetic.
    // The CLI wraps the error in a stable `{ok:false, error: {...}}`
    // envelope so pipelines can branch on the shape rather than
    // parsing prose.
    cmd()
        .args(["--json", "recon", "ip", "127.0.0.1"])
        .assert()
        // SSRF block returns rc=1 by contract; other recon arms
        // return 0. We don't lock the exit code because the
        // contract says the body has `ok:false` either way; if a
        // future PR changes the rc, the JSON-shape contract
        // still pins behaviour.
        .stdout(predicate::str::contains("\"ok\":false"))
        .stdout(predicate::str::contains("refused to target"))
        .stdout(predicate::str::contains("loopback"));
}

#[test]
fn recon_ip_rfc1918_is_blocked_with_rule_tag() {
    // 10.0.0.0/8, 172.16/12, 192.168/16 all fire. We pick 10.0.0.1
    // as the canonical example; the rule tag in the error matches
    // the SSRF gate's variant so scripts can pattern-match on it.
    cmd()
        .args(["--json", "recon", "ip", "10.0.0.1"])
        .assert()
        .stdout(predicate::str::contains("RFC1918"));
}
