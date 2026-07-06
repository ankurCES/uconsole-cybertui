//! One module per CLI subcommand. The dispatcher in `lib.rs` matches the
//! `Cmd` enum against these. Verb mapping is 1:1 with `cyberdeck-daemon`'s
//! `Method` enum; see `commands/daemon.rs` for the canonical pattern.

pub mod audio;
pub mod bluetooth;
pub mod completion;
pub mod config_cmd;
pub mod daemon;
pub mod display;
pub mod logs;
pub mod net;
pub mod packages;
pub mod power;
pub mod process;
pub mod services;
pub mod storage;
pub mod system;
pub mod update;
pub mod workspace;
pub mod wm;
