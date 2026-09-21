//! TUI-03 — the TUI-local bounded ring buffer (plan `PLAN-TUI-PARITY` §3).
//!
//! A self-contained mirror of the GUI `history.rs` ring core (the
//! dashboard trend/history ring): identical fixed-capacity FIFO
//! semantics, re-implemented TUI-local in `std`-only code. The TUI must
//! not depend on `ramsleuth-gui` (plan §1 dependency facts — the
//! egui/eframe pull would be the wrong direction), and this module adds
//! no new dependency.
//!
//! - [`RingBuffer<T>`] — a fixed-capacity FIFO (default
//!   [`RING_CAPACITY`] = 300 samples = 10 min at the 2 s poll cadence,
//!   the GUI `HISTORY_CAPACITY` value). `push` appends the newest
//!   sample and evicts the oldest once the buffer is full. Backed by a
//!   pre-allocated `Vec<Option<T>>` that never grows past the capacity
//!   (bounded memory). TUI-05's graphs `GraphState` stores deeper
//!   1800-sample rings via `with_capacity(1800)`.
//!
//! **No-panic contract:** the buffer never panics — the capacity is
//! clamped to ≥ 1 (every index op is a modulo over a nonzero length)
//! and eviction is an in-place overwrite; `clear` keeps the storage
//! for reuse (no reallocation).
//!
//! **Pure core:** no I/O, no TTY, no external crate — the unit tests
//! below run headless. The module is not yet declared in `lib.rs`
//! (TUI-23 wires + re-exports it); TUI-05 (`graphs.rs`) is the first
//! consumer.

/// Default ring depth: 300 samples = 10 minutes at the 2 s poll
/// cadence (the GUI `HISTORY_CAPACITY` value — bounded memory, no
/// unbounded growth).
pub const RING_CAPACITY: usize = 300;

/// A fixed-capacity FIFO ring buffer: `push` appends the newest
/// sample and evicts the oldest once the capacity is reached.
///
/// Generic over `T` (TUI-05 instantiates it with its timestamped
/// graph sample type). The backing store is a pre-allocated
/// `Vec<Option<T>>` sized at construction: **no heap growth past the
/// fixed capacity**. The capacity is clamped to ≥ 1 so every index
/// computation is a modulo over a nonzero length (no-panic contract).
///
/// Invariant: the live samples occupy `data[head .. head + len]`
/// (wrapping at the capacity), each guaranteed `Some`; every other
/// slot is `None`.
#[derive(Debug, Clone)]
pub struct RingBuffer<T> {
    /// Pre-allocated storage (see the type invariant).
    data: Vec<Option<T>>,
    /// Index of the oldest live sample (0 while the buffer is not
    /// full; advances with each evicting push).
    head: usize,
    /// Number of live samples (always ≤ `data.len()`).
    len: usize,
}

impl<T> RingBuffer<T> {
    /// A buffer at the default [`RING_CAPACITY`] depth.
    pub fn new() -> Self {
        Self::with_capacity(RING_CAPACITY)
    }

    /// A buffer at `capacity` (clamped to ≥ 1, see the type docs).
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        // `resize_with` (not `vec![None; n]`): no `T: Clone` bound.
        let mut data: Vec<Option<T>> = Vec::with_capacity(capacity);
        data.resize_with(capacity, || None);
        Self { data, head: 0, len: 0 }
    }

    /// Append `value` as the newest sample. Once the buffer is full
    /// the oldest sample is evicted (its slot overwritten in place —
    /// no reallocation, ever).
    pub fn push(&mut self, value: T) {
        if self.len == self.data.len() {
            // Full: overwrite the oldest slot, then advance the head.
            self.data[self.head] = Some(value);
            self.head = (self.head + 1) % self.data.len();
        } else {
            let idx = (self.head + self.len) % self.data.len();
            self.data[idx] = Some(value);
            self.len += 1;
        }
    }

    /// The number of live samples.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no samples are stored.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether the buffer holds its full capacity.
    pub fn is_full(&self) -> bool {
        self.len == self.data.len()
    }

    /// The fixed capacity (the clamp-≥1 value from construction).
    pub fn capacity(&self) -> usize {
        self.data.len()
    }

    /// The newest sample (the last one pushed), if any.
    pub fn last(&self) -> Option<&T> {
        if self.len == 0 {
            return None;
        }
        // The live window's last slot (the invariant makes it `Some`).
        self.data[(self.head + self.len - 1) % self.data.len()].as_ref()
    }

    /// The live samples, oldest → newest.
    pub fn iter(&self) -> RingIter<'_, T> {
        RingIter {
            data: &self.data,
            idx: self.head,
            remaining: self.len,
        }
    }

    /// Drop every sample (the head rewinds; the storage is kept and
    /// reused — no reallocation).
    pub fn clear(&mut self) {
        for slot in &mut self.data {
            *slot = None;
        }
        self.head = 0;
        self.len = 0;
    }
}

impl<T> Default for RingBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// An iterator over a [`RingBuffer`]'s live samples, oldest → newest.
pub struct RingIter<'a, T> {
    data: &'a [Option<T>],
    idx: usize,
    remaining: usize,
}

impl<'a, T> Iterator for RingIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        // The live window's slot is guaranteed `Some` (the invariant);
        // the `?` is a panic-free tripwire, not a reachable failure.
        let value = self.data[self.idx].as_ref()?;
        self.idx = (self.idx + 1) % self.data.len();
        Some(value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

// ---------------------------------------------------------------------
// Tests (headless: no TTY, no I/O, std only).
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// (a) A fresh buffer is empty, at the default capacity, and not
    /// full; `last` / `iter` report nothing.
    #[test]
    fn new_buffer_is_empty_at_default_capacity() {
        let buf: RingBuffer<f64> = RingBuffer::new();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), RING_CAPACITY);
        assert!(!buf.is_full());
        assert!(buf.last().is_none());
        assert!(buf.iter().next().is_none());
    }

    /// (b) A partial fill: `len` tracks the pushes, `iter` runs
    /// oldest → newest, `last` is the newest, and it is not full.
    #[test]
    fn partial_fill_iter_order_and_last() {
        let mut buf: RingBuffer<u32> = RingBuffer::with_capacity(5);
        for value in [10, 20, 30] {
            buf.push(value);
        }
        assert_eq!(buf.len(), 3);
        assert!(!buf.is_full());
        assert_eq!(buf.last(), Some(&30));
        let order: Vec<u32> = buf.iter().copied().collect();
        assert_eq!(order, vec![10, 20, 30], "iter must run oldest → newest");
    }

    /// (c) Wrap-around: push capacity + K — the oldest K are evicted,
    /// the last capacity remain in order, and the capacity never grows.
    #[test]
    fn wraparound_evicts_the_oldest() {
        let mut buf: RingBuffer<u32> = RingBuffer::with_capacity(5);
        for value in 0..8 {
            buf.push(value); // 3 past the capacity
        }
        assert!(buf.is_full());
        assert_eq!(buf.capacity(), 5, "the capacity must never grow");
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.last(), Some(&7));
        assert_eq!(
            buf.iter().copied().collect::<Vec<u32>>(),
            vec![3, 4, 5, 6, 7],
            "the oldest three (0, 1, 2) must be evicted, in order"
        );
    }

    /// (d) The default 300-capacity buffer: push 350 — the first 50
    /// are evicted, 300 remain (10 min at the 2 s poll).
    #[test]
    fn default_capacity_wrap() {
        let mut buf: RingBuffer<f64> = RingBuffer::new();
        for value in 0..350 {
            buf.push(f64::from(value));
        }
        assert!(buf.is_full());
        assert_eq!(buf.len(), RING_CAPACITY);
        assert_eq!(buf.last(), Some(&349.0));
        assert_eq!(buf.iter().next(), Some(&50.0), "the 50 oldest must be evicted");
    }

    /// (e) `clear` drops every sample (the head rewinds) and pushes
    /// after it still wrap correctly — the freed slots are reused, no
    /// growth.
    #[test]
    fn clear_resets_and_reuse_wraps() {
        let mut buf: RingBuffer<u32> = RingBuffer::with_capacity(3);
        for value in [1, 2, 3, 4] {
            buf.push(value); // full; the head has advanced
        }
        assert!(buf.is_full());
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert!(buf.last().is_none());
        assert!(buf.iter().next().is_none());
        // Fill past the capacity again: the freed slots must be
        // reused without a reallocation.
        for value in [9, 8, 7, 6] {
            buf.push(value);
        }
        assert!(buf.is_full());
        assert_eq!(buf.last(), Some(&6));
        assert_eq!(
            buf.iter().copied().collect::<Vec<u32>>(),
            vec![8, 7, 6]
        );
    }

    /// (f) A zero capacity is clamped to one (no modulo-by-zero —
    /// the no-panic contract).
    #[test]
    fn zero_capacity_is_clamped() {
        let mut buf: RingBuffer<u8> = RingBuffer::with_capacity(0);
        assert_eq!(buf.capacity(), 1);
        buf.push(1);
        buf.push(2);
        assert!(buf.is_full());
        assert_eq!(buf.last(), Some(&2));
        assert_eq!(buf.iter().count(), 1);
    }

    /// (g) Iterator bounds: `size_hint` is exact (lo == hi ==
    /// remaining), the count is exactly `len`, and the (len + 1)th
    /// element is `None` — no over-reading past the live window.
    #[test]
    fn iter_bounds_match_len_and_size_hint() {
        let mut buf: RingBuffer<u32> = RingBuffer::with_capacity(4);
        for value in [7, 8, 9] {
            buf.push(value);
        }
        let mut iter = buf.iter();
        assert_eq!(iter.size_hint(), (3, Some(3)));
        assert_eq!(iter.next(), Some(&7));
        assert_eq!(iter.size_hint(), (2, Some(2)));
        assert_eq!(iter.next(), Some(&8));
        assert_eq!(iter.next(), Some(&9));
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.next(), None, "the (len + 1)th element must be None");
        assert_eq!(iter.next(), None, "an exhausted iterator stays exhausted");
    }

    /// (h) The live window crossing the physical end of the backing
    /// store: fill capacity + K, the head is no longer 0, and `iter`
    /// wraps over the index-0 seam in oldest → newest order.
    #[test]
    fn iter_wraps_across_the_physical_seam() {
        let mut buf: RingBuffer<u32> = RingBuffer::with_capacity(4);
        for value in 0..6 {
            buf.push(value); // full; the head has moved to slot 2
        }
        assert_eq!(
            buf.iter().copied().collect::<Vec<u32>>(),
            vec![2, 3, 4, 5],
            "the window spans slots 2, 3, 0, 1 across the physical seam"
        );
    }
}
