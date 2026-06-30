#!/usr/bin/env bash
# Reminder hook: when the user runs `cargo test`, suggest `scripts/safe-test` instead.
# Install via the line in CONTRIBUTING.md (eval "$(cat scripts/sh/cargo-test-with-safe-test.bash)").
# Does NOT block cargo test — it's a friendly reminder.

_cargo_test_reminder() {
    local current_command="${BASH_COMMAND}"
    # Detect bare `cargo test` or `cargo test ...` (but not `safe-test` or wrappers).
    if [[ "$current_command" =~ ^cargo[[:space:]]+test($|[[:space:]]) ]]; then
        # Don't fire if the user explicitly called scripts/safe-test or another wrapper.
        if [[ "$current_command" != *"safe-test"* ]]; then
            printf '\033[33mreminder:\033[0m prefer \033[1mscripts/safe-test\033[0m over bare cargo test (caps wall time, no hangs)\n' >&2
        fi
    fi
}

# Only install the trap if not already installed (idempotent).
if [[ -z "$_CARGO_TEST_REMINDER_INSTALLED" ]]; then
    trap '_cargo_test_reminder' DEBUG
    export _CARGO_TEST_REMINDER_INSTALLED=1
fi