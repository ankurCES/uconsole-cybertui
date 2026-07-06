//! Agent-state classification: look at the most-recent N bytes of a pane's
//! PTY output and classify the pane as Blocked / Working / Done / Idle / Unknown.
//!
//! ## Design
//!
//! The classifier is intentionally simple — a fixed set of regex patterns
//! against the tail of the pane. It's a synchronous, pure function that takes
//! the previous state into account so we don't flap from Working to Done on
//! the next byte that happens to match.
//!
//! ## State transitions
//!
//! - `Blocked`: anything matching a "needs human input" pattern.
//! - `Working`: anything matching a "in progress" pattern (progress %, "Building", ...).
//! - `Done`:    a "completed" pattern AND we were previously Working
//!              (avoids false positives on a fresh shell prompt).
//! - otherwise: hold the previous state. (Idle and Unknown are sticky.)

use once_cell::sync::Lazy;
use regex::Regex;

use crate::state::PaneState;

/// How many trailing lines of PTY output to inspect. 20 is a comfortable
/// tail that catches multi-line prompts (e.g. heredocs) without scanning
/// megabytes of scrollback.
const TAIL_LINES: usize = 20;

/// Patterns that mean the pane needs human input RIGHT NOW.
static BLOCKED_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"(?i)password:").expect("BLOCKED regex compiles"),
        Regex::new(r"(?i)passphrase:").expect("BLOCKED regex compiles"),
        Regex::new(r"(?i)\(y/n\)").expect("BLOCKED regex compiles"),
        Regex::new(r"(?i)\[Y/n\]").expect("BLOCKED regex compiles"),
        Regex::new(r"(?i)continue\?\s*\(y/n\)").expect("BLOCKED regex compiles"),
        Regex::new(r"\$ \(\$\) ").expect("BLOCKED regex compiles"),
        Regex::new(r"(?i)permission denied").expect("BLOCKED regex compiles"),
        Regex::new(r"(?i)are you sure").expect("BLOCKED regex compiles"),
    ]
});

/// Patterns that mean the pane is actively producing output.
static WORKING_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"\.\.\. \d+%").expect("WORKING regex compiles"),
        Regex::new(r"\b\d+%\b").expect("WORKING regex compiles"),
        Regex::new(r"(?i)\bbuilding\b").expect("WORKING regex compiles"),
        Regex::new(r"(?i)\bdownloading\b").expect("WORKING regex compiles"),
        Regex::new(r"(?i)\bcompiling\b").expect("WORKING regex compiles"),
        Regex::new(r"(?i)\bupgrading\b").expect("WORKING regex compiles"),
        Regex::new(r"(?i)\binstalling\b").expect("WORKING regex compiles"),
        Regex::new(r"(?i)\bconnecting\b").expect("WORKING regex compiles"),
    ]
});

/// Patterns that mean the pane just finished a task. Only transitions
/// from Working → Done to avoid spuriously classifying a fresh shell prompt
/// as Done when the previous state was Unknown or Idle.
static DONE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"(?i)build succeeded").expect("DONE regex compiles"),
        Regex::new(r"(?i)test[s]? passed").expect("DONE regex compiles"),
        Regex::new(r"(?i)\bcomplete\b").expect("DONE regex compiles"),
        Regex::new(r"(?i)finished in \d").expect("DONE regex compiles"),
        Regex::new(r"\$ $").expect("DONE regex compiles"), // bare prompt at end
    ]
});

/// Classify the most-recent portion of `text` (the pane's PTY output) into
/// a [`PaneState`]. `prev` is the pane's state before this chunk arrived.
///
/// The function is pure: no I/O, no async, no global state. Same input →
/// same output, modulo `prev` which is supplied by the caller.
pub fn classify(text: &str, prev: PaneState) -> PaneState {
    // Empty input: hold previous state. Don't transition to Done/Idle on
    // silence — that would mask Working panes that are mid-build.
    if text.is_empty() {
        return prev;
    }

    // Tail-rolling: look at the last 20 lines. Cheap and avoids false positives
    // from old output higher up.
    let tail: String = text
        .lines()
        .rev()
        .take(TAIL_LINES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");

    if BLOCKED_PATTERNS.iter().any(|r| r.is_match(&tail)) {
        return PaneState::Blocked;
    }
    if WORKING_PATTERNS.iter().any(|r| r.is_match(&tail)) {
        return PaneState::Working;
    }
    // Done is only a valid transition from Working — prevents stale
    // "complete" messages from the install history looking like a fresh finish.
    if prev == PaneState::Working && DONE_PATTERNS.iter().any(|r| r.is_match(&tail)) {
        return PaneState::Done;
    }
    // No pattern matched (or matched but state-machine forbids the transition).
    // Hold the previous state.
    prev
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pattern -> State lookup helpers keep the tests readable.
    fn blocked(t: &str) -> PaneState {
        classify(t, PaneState::Unknown)
    }
    fn working(t: &str) -> PaneState {
        classify(t, PaneState::Unknown)
    }

    #[test]
    fn classify_detects_password_prompt_as_blocked() {
        assert_eq!(
            blocked("Login: alice\nPassword: "),
            PaneState::Blocked
        );
        // case-insensitive
        assert_eq!(
            blocked("sudo password: "),
            PaneState::Blocked
        );
        // passphrase variant
        assert_eq!(
            blocked("Enter passphrase: "),
            PaneState::Blocked
        );
    }

    #[test]
    fn classify_detects_yn_prompt_as_blocked() {
        assert_eq!(
            blocked("Do you want to continue? (y/n) "),
            PaneState::Blocked
        );
        assert_eq!(
            blocked("Proceed? [Y/n] "),
            PaneState::Blocked
        );
    }

    #[test]
    fn classify_detects_permission_denied_as_blocked() {
        // useful for surfacing failed sudo without forcing the user to scroll
        assert_eq!(
            blocked("bash: /usr/local/bin/foo: Permission denied"),
            PaneState::Blocked
        );
    }

    #[test]
    fn classify_detects_progress_percent_as_working() {
        assert_eq!(
            working("Resolving packages... 47%"),
            PaneState::Working
        );
        assert_eq!(
            working("Downloading linux-firmware... 12%"),
            PaneState::Working
        );
        // bare percent
        assert_eq!(working("Build 60%"), PaneState::Working);
    }

    #[test]
    fn classify_detects_active_keywords_as_working() {
        assert_eq!(
            working("Building foo v1.2"),
            PaneState::Working
        );
        assert_eq!(
            working("Downloading linux-firmware"),
            PaneState::Working
        );
        assert_eq!(
            working("Compiling dependency tree..."),
            PaneState::Working
        );
    }

    #[test]
    fn classify_detects_done_after_working() {
        // Was Working, now sees "build succeeded" -> Done.
        let prev = PaneState::Working;
        let next = classify(
            "Compiling foo v0.1\nBuild succeeded in 1m 32s\n$ ",
            prev,
        );
        assert_eq!(next, PaneState::Done);
    }

    #[test]
    fn classify_does_not_transition_to_done_from_unknown() {
        // Fresh pane, sees "complete" — should NOT go straight to Done.
        let next = classify("install complete", PaneState::Unknown);
        assert_eq!(next, PaneState::Unknown);
    }

    #[test]
    fn classify_does_not_transition_to_done_from_idle() {
        // Idle pane that suddenly mentions "complete" — should hold Idle.
        let next = classify("apt-get: complete", PaneState::Idle);
        assert_eq!(next, PaneState::Idle);
    }

    #[test]
    fn classify_holds_state_when_no_pattern_matches() {
        // Random shell chatter that's neither building nor blocked.
        let prev = PaneState::Idle;
        let next = classify("ls -la\nfoo bar baz\n$ ", prev);
        assert_eq!(next, prev);
    }

    #[test]
    fn classify_handles_empty_text() {
        assert_eq!(classify("", PaneState::Idle), PaneState::Idle);
        assert_eq!(classify("", PaneState::Working), PaneState::Working);
        assert_eq!(classify("", PaneState::Unknown), PaneState::Unknown);
    }

    #[test]
    fn classify_only_inspects_tail_not_full_scrollback() {
        // Old "100%" deep in scrollback must not flip us to Working
        // if the tail is just a quiet prompt.
        let text = "Downloading... 100%\n[old line]\n[old line]\n$ ";
        // The tail includes the bare-prompt match which could match DONE —
        // but prev is Unknown, so we hold Unknown.
        let next = classify(text, PaneState::Unknown);
        assert_eq!(next, PaneState::Unknown);
    }

    #[test]
    fn classify_blocks_takes_priority_over_working() {
        // Even if there's a progress indicator in the output, an active
        // password prompt is what the user needs to see first.
        let next = classify(
            "Resolving packages... 47%\n[sudo] password for alice: ",
            PaneState::Working,
        );
        assert_eq!(next, PaneState::Blocked);
    }

    #[test]
    fn classify_working_takes_priority_over_done() {
        // New build kicked off right after a "build succeeded" — Working wins.
        let next = classify(
            "build succeeded\nBuilding new artifact...\nCompiling",
            PaneState::Working,
        );
        assert_eq!(next, PaneState::Working);
    }
}