//! Daemon process: hosts workspace state and serves JSON-RPC over a
//! local socket. Both the TUI and the CLI connect to it.

pub mod rpc;
pub mod socket;

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("rpc: {0}")] Rpc(String),
}

pub type DaemonResult<T> = std::result::Result<T, DaemonError>;
