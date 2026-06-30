//! Lint tests for the `cargo test` reminder hook at
//! `scripts/sh/cargo-test-with-safe-test.bash`.
//!
//! The hook is a small Bash snippet installed into the user's interactive
//! shell. It uses a DEBUG trap, so a syntax error there would silently
//! break the user's prompt — hence these tests:
//!
//!   1. `bash -n` parses cleanly (no syntax errors).
//!   2. The script has the required surface: shebang, DEBUG trap, and the
//!      idempotency env var (so installing it twice doesn't stack traps).
//!
//! Run with: `scripts/safe-test -p cyberdeck-web --test safe_test_hook`.

use std::path::PathBuf;
use std::process::Command;

/// Resolve the workspace root from `CARGO_MANIFEST_DIR` (set at compile time
/// to the crate that owns this integration test). For `cyberdeck-web` the
/// manifest dir is `crates/web/`, so the workspace root is two levels up.
fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/web -> ../..
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn hook_path() -> PathBuf {
    workspace_root().join("scripts/sh/cargo-test-with-safe-test.bash")
}

#[test]
fn cargo_test_reminder_hook_parses_with_bash_n() {
    let hook = hook_path();
    let script = std::fs::read_to_string(&hook)
        .expect("hook script must exist at scripts/sh/cargo-test-with-safe-test.bash");
    let tmp = std::env::temp_dir().join("cargo-test-hook-lint.bash");
    std::fs::write(&tmp, &script).expect("write tmp");

    let output = Command::new("bash")
        .arg("-n")
        .arg(&tmp)
        .output()
        .expect("bash must be installed");
    assert!(
        output.status.success(),
        "bash -n failed:\nstderr: {}\nscript:\n{}",
        String::from_utf8_lossy(&output.stderr),
        script,
    );
}

#[test]
fn cargo_test_reminder_hook_has_shebang_and_trap() {
    let hook = hook_path();
    let script = std::fs::read_to_string(&hook)
        .expect("hook script must exist at scripts/sh/cargo-test-with-safe-test.bash");
    assert!(script.starts_with("#!/usr/bin/env bash"), "must have shebang");
    assert!(script.contains("trap '"), "must install a DEBUG trap");
    assert!(
        script.contains("_CARGO_TEST_REMINDER_INSTALLED"),
        "must be idempotent via env var"
    );
}