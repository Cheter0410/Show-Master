//! Fixed-capacity, allocation-free queue of pending transports sorted by
//! anchor sample. Shared by the LTC encoder and the MTC generator.

use crate::types::{FrameRate, TcTransport};

/// Maximum number of transports with future anchors that can be queued.
pub const PENDING_CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug)]
pub(crate) struct TransportQueue {
    items: [TcTransport; PENDING_CAPACITY],
    len: usize,
}

impl TransportQueue {
    pub(crate) fn new() -> Self {
        Self {
            items: [TcTransport::stopped(FrameRate::F25); PENDING_CAPACITY],
            len: 0,
        }
    }

    pub(crate) fn first(&self) -> Option<TcTransport> {
        if self.len == 0 {
            None
        } else {
            self.items.first().copied()
        }
    }

    pub(crate) fn pop_front(&mut self) {
        if self.len == 0 {
            return;
        }
        if let Some(live) = self.items.get_mut(..self.len) {
            live.rotate_left(1);
        }
        self.len -= 1;
    }

    /// Insert keeping the queue sorted by anchor. An entry with the same
    /// anchor is replaced. When the queue is full the entry with the latest
    /// anchor is discarded to make room (or overwritten, if the new entry is
    /// itself the latest).
    pub(crate) fn insert(&mut self, t: TcTransport) {
        let len = self.len.min(PENDING_CAPACITY);
        let mut pos = self
            .items
            .iter()
            .take(len)
            .take_while(|p| p.anchor_sample < t.anchor_sample)
            .count();
        if pos < len {
            if let Some(slot) = self.items.get_mut(pos) {
                if slot.anchor_sample == t.anchor_sample {
                    *slot = t;
                    return;
                }
            }
        }
        let new_len = if len == PENDING_CAPACITY {
            pos = pos.min(PENDING_CAPACITY - 1);
            len
        } else {
            len + 1
        };
        if let Some(tail) = self.items.get_mut(pos..new_len) {
            tail.rotate_right(1);
            if let Some(slot) = tail.first_mut() {
                *slot = t;
            }
        }
        self.len = new_len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(anchor: u64, frame: u64) -> TcTransport {
        TcTransport {
            rate: FrameRate::F25,
            running: true,
            anchor_sample: anchor,
            anchor_frame: frame,
        }
    }

    fn drain(q: &mut TransportQueue) -> Vec<(u64, u64)> {
        let mut v = Vec::new();
        while let Some(p) = q.first() {
            v.push((p.anchor_sample, p.anchor_frame));
            q.pop_front();
        }
        v
    }

    #[test]
    fn sorted_insert_and_replace() {
        let mut q = TransportQueue::new();
        q.insert(t(30, 0));
        q.insert(t(10, 1));
        q.insert(t(20, 2));
        q.insert(t(10, 3));
        assert_eq!(drain(&mut q), vec![(10, 3), (20, 2), (30, 0)]);
        assert!(q.first().is_none());
    }

    #[test]
    fn overflow_discards_latest() {
        let mut q = TransportQueue::new();
        for i in 0..PENDING_CAPACITY as u64 {
            q.insert(t(100 + i * 10, i));
        }
        q.insert(t(105, 99)); // pushes out the last one
        q.insert(t(10_000, 77)); // overwrites the last one
        let v = drain(&mut q);
        assert_eq!(v.len(), PENDING_CAPACITY);
        assert_eq!(v[1], (105, 99));
        assert_eq!(v[PENDING_CAPACITY - 1], (10_000, 77));
        assert!(v.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
