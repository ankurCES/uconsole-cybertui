//! Non-blocking toast notifications. Same model for the TUI and the web.

use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    pub created: Instant,
    pub ttl_secs: u64,
}

impl Toast {
    pub fn new(kind: ToastKind, text: String) -> Self {
        Self {
            kind,
            text,
            created: Instant::now(),
            ttl_secs: 3,
        }
    }
    #[allow(dead_code)] // builder; used by screens once TTLs are configurable
    pub fn with_ttl(mut self, secs: u64) -> Self {
        self.ttl_secs = secs;
        self
    }
    pub fn expired(&self) -> bool {
        self.created.elapsed().as_secs() > self.ttl_secs
    }
}
