//! A simple bounded ring buffer of `f32` samples, shared between the capture thread (writer) and
//! the VAD / worker (reader/drainer).
//!
//! It stores a rolling window of the most recent `capacity` samples. Writes never block and never
//! allocate after construction; when full, the oldest samples are overwritten. A monotonically
//! increasing absolute sample index lets consumers reason about positions across overwrites.

use parking_lot::Mutex;
use std::sync::Arc;

/// Thread-safe rolling buffer of recent audio.
#[derive(Clone)]
pub struct RingBuffer {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    buf: Vec<f32>,
    capacity: usize,
    /// Total samples ever written (absolute index of the next write).
    written: u64,
}

impl RingBuffer {
    /// Create a ring buffer holding at most `capacity` samples (e.g. 60 s * 16 kHz).
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                buf: Vec::with_capacity(capacity),
                capacity: capacity.max(1),
                written: 0,
            })),
        }
    }

    /// Append samples (called from the capture thread). Oldest data is dropped when full.
    pub fn push(&self, samples: &[f32]) {
        let mut g = self.inner.lock();
        g.written += samples.len() as u64;
        let cap = g.capacity;
        if samples.len() >= cap {
            g.buf.clear();
            g.buf.extend_from_slice(&samples[samples.len() - cap..]);
            return;
        }
        let overflow = (g.buf.len() + samples.len()).saturating_sub(cap);
        if overflow > 0 {
            g.buf.drain(0..overflow);
        }
        g.buf.extend_from_slice(samples);
    }

    /// Snapshot the current contents (oldest → newest).
    pub fn snapshot(&self) -> Vec<f32> {
        self.inner.lock().buf.clone()
    }

    /// Number of samples currently buffered.
    pub fn len(&self) -> usize {
        self.inner.lock().buf.len()
    }

    /// Whether the buffer currently holds no samples.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().buf.is_empty()
    }

    /// Total samples ever written (survives overwrites); useful as an absolute timeline.
    pub fn total_written(&self) -> u64 {
        self.inner.lock().written
    }

    /// Clear the buffer and reset the absolute counter (call at the start of a new recording).
    pub fn reset(&self) {
        let mut g = self.inner.lock();
        g.buf.clear();
        g.written = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_snapshot() {
        let rb = RingBuffer::new(8);
        rb.push(&[1.0, 2.0, 3.0]);
        assert_eq!(rb.snapshot(), vec![1.0, 2.0, 3.0]);
        assert_eq!(rb.len(), 3);
    }

    #[test]
    fn overwrites_oldest_when_full() {
        let rb = RingBuffer::new(4);
        rb.push(&[1.0, 2.0, 3.0, 4.0]);
        rb.push(&[5.0, 6.0]);
        assert_eq!(rb.snapshot(), vec![3.0, 4.0, 5.0, 6.0]);
        assert_eq!(rb.total_written(), 6);
    }

    #[test]
    fn push_larger_than_capacity_keeps_tail() {
        let rb = RingBuffer::new(3);
        rb.push(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(rb.snapshot(), vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn reset_clears() {
        let rb = RingBuffer::new(4);
        rb.push(&[1.0, 2.0]);
        rb.reset();
        assert!(rb.is_empty());
        assert_eq!(rb.total_written(), 0);
    }
}
