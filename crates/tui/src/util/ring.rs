//! Small fixed-capacity ring buffer of `u64` values. Module 5.2 backs
//! `Live::net_history` with one ring per (interface, direction) so the
//! header sparkline can show the last 60 seconds of byte deltas without
//! growing memory unbounded.
//!
//! `RingU64` is intentionally tiny: no `unsafe`, no `no_std` tricks, no
//! `Default` (zero capacity is allowed and is a no-op, but isn't a sane
//! default for a UI history). Most callers want `RingU64::new(60)`.
//!
//! Chronology: `as_slice_chrono` returns elements oldest-to-newest. The
//! 1Hz refiller in `Live::spawn_refreshers` only appends, so chronological
//! order is also insertion order — no per-element timestamp needed.
//! Sparkline renderers read the tail (`.iter().rev().take(N).collect()`
//! then reverse) to show "last N samples in chronological order".
#[derive(Debug, Clone)]
pub struct RingU64 {
    buf: Vec<u64>,
    cap: usize,
    /// Index of the oldest element. `0` until the first overflow.
    start: usize,
    /// Number of live elements. Capped at `cap`.
    len: usize,
}

impl RingU64 {
    /// Create a ring buffer with the given capacity. `new(0)` is legal
    /// and turns `push` into a no-op — useful as a fallback when the
    /// caller wants a uniform type without special-casing zero.
    pub fn new(cap: usize) -> Self {
        Self {
            buf: vec![0; cap],
            cap,
            start: 0,
            len: 0,
        }
    }

    /// Append `v`. When the ring is full, the oldest element is dropped
    /// and `start` advances by one. `O(1)`; never reallocates after
    /// construction.
    pub fn push(&mut self, v: u64) {
        if self.cap == 0 {
            return;
        }
        let idx = (self.start + self.len) % self.cap;
        if self.len < self.cap {
            self.buf[idx] = v;
            self.len += 1;
        } else {
            // Full ring: write over the slot start points at, then
            // bump start so the next overwrite lands on the new oldest.
            self.buf[idx] = v;
            self.start = (self.start + 1) % self.cap;
        }
    }

    /// Snapshot the contents as a `Vec<u64>` in chronological order
    /// (oldest first). Allocates a fresh `Vec` each call; for hot
    /// render paths prefer the slice/iterator view (TBD if perf
    /// matters in practice — the cap is 60 elements either way).
    pub fn as_slice_chrono(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.len);
        for i in 0..self.len {
            out.push(self.buf[(self.start + i) % self.cap]);
        }
        out
    }

    /// Number of live elements. Always `<= cap`.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when no element has been pushed (or the ring has zero cap).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Configured capacity. Constant after construction.
    pub fn cap(&self) -> usize {
        self.cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_push_and_iter_chronological() {
        let mut r = RingU64::new(3);
        r.push(10);
        r.push(20);
        r.push(30);
        assert_eq!(r.as_slice_chrono(), vec![10, 20, 30]);
    }

    #[test]
    fn ring_overflow_drops_oldest() {
        let mut r = RingU64::new(3);
        r.push(1);
        r.push(2);
        r.push(3);
        r.push(4);
        r.push(5);
        // Capacity is 3; the two oldest (1, 2) are gone, 3/4/5 remain
        // in insertion order.
        assert_eq!(r.as_slice_chrono(), vec![3, 4, 5]);
    }

    #[test]
    fn ring_zero_cap_is_noop() {
        let mut r = RingU64::new(0);
        r.push(99);
        assert_eq!(r.as_slice_chrono(), Vec::<u64>::new());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn ring_len_tracks_filled_then_caps() {
        let mut r = RingU64::new(3);
        assert_eq!(r.len(), 0);
        assert!(r.is_empty());
        r.push(1);
        assert_eq!(r.len(), 1);
        r.push(2);
        r.push(3);
        assert_eq!(r.len(), 3);
        r.push(4);
        // Saturation: len never exceeds cap, even after wraparound.
        assert_eq!(r.len(), 3);
    }
}
