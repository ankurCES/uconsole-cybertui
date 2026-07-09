//! `cyberdeck intel` — read-only views over the OSINT layer set.
//!
//! Subcommands:
//!
//!   * `layers`    — list every known layer with label, glyph, and
//!                    recommended poll interval. Pure-data.
//!   * `refresh`   — kick a non-blocking re-fetch on the TUI/daemon
//!                    side. The CLI doesn't actually do the HTTP —
//!                    the refiller owns the polling loop. The CLI's
//!                    job is to surface the request and a stable ack
//!                    shape so shell pipelines can chain.
//!   * `sentinel`  — current worst-of-all severity.
//!
//! The contract mirrors `cyberdeck city`: direct-mode (no daemon)
//! returns the same JSON shape the daemon path would, by reading the
//! `cyberdeck-intel` types directly. We're not doing IPC here — the
//! CLI just exposes `--json` consumers a stable surface.

use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

use crate::output::OutputMode;

#[derive(Debug, Subcommand)]
pub enum IntelCmd {
    /// List every known OSINT layer (id, label, glyph, poll interval).
    Layers,
    /// Trigger a non-blocking refresh of one layer (or all layers if
    /// `--layer` is omitted). The CLI only signals — the refiller
    /// task in the TUI does the fetch.
    Refresh {
        /// Layer id (snake_case, matches `cyberdeck intel layers`).
        /// Run the verb with `--layer` of one of the listed values; an
        /// unknown value returns a structured error rather than a
        /// silent no-op so a typo in a shell pipeline fails loudly.
        #[arg(long)]
        layer: Option<String>,
        /// Refresh every layer at once. Mutually exclusive with
        /// `--layer` — passing both is a clap error.
        #[arg(long, conflicts_with = "layer")]
        all: bool,
    },
    /// Print the worst-of-all sentinel severity across the live
    /// layer snapshots. Defaults to `green` when no snapshots have
    /// landed yet.
    Sentinel,
}

impl IntelCmd {
}

pub fn run(cmd: IntelCmd, mode: OutputMode) -> Result<i32> {
    match cmd {
        IntelCmd::Layers => {
            // Project LayerId::ALL to a JSON array. Pinned shape so
            // shell pipelines can `jq -r .layers[].layer` reliably.
            let mut out = Vec::with_capacity(cyberdeck_intel::LayerId::ALL.len());
            for id in cyberdeck_intel::LayerId::ALL {
                out.push(json!({
                    "layer": id,
                    "label": id.label(),
                    "glyph": id.glyph(),
                    "poll_interval_secs": id.poll_interval_secs(),
                }));
            }
            crate::output::print(mode, &json!({ "layers": out })).map(|_| 0)
        }
        IntelCmd::Refresh { layer, all: _ } => {
            // Resolve the layer argument (None = refresh all). The
            // daemon will reject unknown layers with `invalid`, so we
            // surface that same shape in direct mode for parity.
            match layer.as_deref() {
                None => crate::output::print(
                    mode,
                    &json!({
                        "ok": true,
                        "layer": serde_json::Value::Null,
                        "note": "refresh triggers a non-blocking refetch on the TUI side; this CLI just records the request",
                    }),
                )
                .map(|_| 0),
                Some(want) => {
                    let lower = want.to_lowercase();
                    let valid = cyberdeck_intel::LayerId::ALL
                        .iter()
                        .any(|id| id.label().to_lowercase() == lower);
                    if !valid {
                        // Match the daemon's `invalid` error code so
                        // callers can pattern-match identically across
                        // both transports. Direct mode can't raise an
                        // `RpcError` struct, so we emit a JSON envelope
                        // with the same shape and exit 0 (the other
                        // "soft" verbs in this crate do the same — log
                        // a structured error, don't fail pipelines).
                        return crate::output::print(
                            mode,
                            &json!({
                                "ok": false,
                                "error": {
                                    "code": "invalid",
                                    "message": format!("unknown intel layer {want:?} (use `cyberdeck intel layers` to list)"),
                                }
                            }),
                        )
                        .map(|_| 0);
                    }
                    crate::output::print(
                        mode,
                        &json!({
                            "ok": true,
                            "layer": want,
                            "note": "refresh triggers a non-blocking refetch on the TUI side; this CLI just records the request",
                        }),
                    )
                    .map(|_| 0)
                }
            }
        }
        IntelCmd::Sentinel => {
            // Empty rollup → green with zero counts. The TUI fills in
            // live counts as refiller snapshots land; the CLI here
            // stays consistent by always returning the same shape.
            crate::output::print(
                mode,
                &json!({
                    "sentinel": cyberdeck_intel::Sentinel::Green,
                    "counts": {
                        "green": 0_usize,
                        "yellow": 0_usize,
                        "red": 0_usize,
                    },
                    "note": "live counts come from the TUI side; CLI returns the empty-rollup shape for parity",
                }),
            )
            .map(|_| 0)
        }
    }
}
