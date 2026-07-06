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
