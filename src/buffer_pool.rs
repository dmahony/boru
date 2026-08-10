//! Zero-allocation byte-buffer pooling.
//!
//! A [`BufferPool`](crate::buffer_pool::BufferPool) holds a fixed-size
//! collection of pre-allocated byte buffers.
//! [`BufferPool::acquire`](crate::buffer_pool::BufferPool::acquire) hands out
//! one buffer at a time wrapped in a
//! [`PooledBuffer`](crate::buffer_pool::PooledBuffer); calling
//! [`PooledBuffer::freeze`](crate::buffer_pool::PooledBuffer::freeze) turns it
//! into an immutable, reference-counted
//! [`PooledBytes`](crate::buffer_pool::PooledBytes).  When the last
//! [`PooledBytes`](crate::buffer_pool::PooledBytes) reference is dropped, the
//! underlying allocation is cleared and returned to the pool, so the steady
//! state performs no heap allocation for repeated message construction.
//!
//! The pool never panics on exhaustion: if no idle buffer is available,
//! [`BufferPool::acquire`](crate::buffer_pool::BufferPool::acquire) falls back
//! to a fresh heap allocation, and buffers returned while the pool is already
//! full (or still shared with cloned [`Bytes`](bytes::Bytes)) are freed
//! normally.  Returned buffers keep whatever capacity they grew to while in
//! use, so a buffer that outgrew its nominal
//! [`BufferPool`](crate::buffer_pool::BufferPool) capacity stays large for
//! subsequent reuse.

use bytes::Bytes;
use std::sync::{Mutex, MutexGuard};

/// A fixed-size pool of pre-allocated byte buffers.
///
/// Thread-safe: all access to the idle-buffer list is serialized behind a
/// [`Mutex`].
#[derive(Debug)]
pub struct BufferPool {
    /// Maximum number of buffers retained by the pool.
    pool_size: usize,
    /// Initial capacity of freshly allocated buffers.
    capacity: usize,
    /// Idle buffers available for reuse.
    buffers: Mutex<Vec<Vec<u8>>>,
}

impl BufferPool {
    /// Create a new pool that retains at most `pool_size` buffers, each freshly
    /// allocated with `capacity` bytes of capacity.
    pub fn new(pool_size: usize, capacity: usize) -> Self {
        Self {
            pool_size,
            capacity,
            buffers: Mutex::new(Vec::new()),
        }
    }

    /// Acquire a buffer from the pool.
    ///
    /// If the pool holds an idle buffer it is reused; otherwise a fresh buffer
    /// is allocated from the heap.  This never blocks and never panics when the
    /// pool is exhausted.
    pub fn acquire(&self) -> PooledBuffer<'_> {
        let buf = self
            .buffers()
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(self.capacity));
        PooledBuffer { pool: self, buf }
    }

    /// Return a buffer to the pool, clearing it and dropping it if the pool is
    /// already full.
    fn release(&self, mut buf: Vec<u8>) {
        buf.clear();
        let mut buffers = self.buffers();
        if buffers.len() < self.pool_size {
            buffers.push(buf);
        }
    }

    /// Lock the idle-buffer list, recovering from a poisoned mutex instead of
    /// panicking.
    fn buffers(&self) -> MutexGuard<'_, Vec<Vec<u8>>> {
        self.buffers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A mutable byte buffer acquired from a [`BufferPool`].
///
/// Write into it with [`inner_mut`](Self::inner_mut), then hand the finished
/// bytes off with [`freeze`](Self::freeze).  Dropping a [`PooledBuffer`]
/// without freezing simply frees the buffer.
#[derive(Debug)]
pub struct PooledBuffer<'a> {
    pool: &'a BufferPool,
    buf: Vec<u8>,
}

impl<'a> PooledBuffer<'a> {
    /// Mutable access to the underlying buffer for writing.
    pub fn inner_mut(&mut self) -> &mut Vec<u8> {
        &mut self.buf
    }

    /// Freeze this buffer into an immutable, shareable [`PooledBytes`].
    ///
    /// The allocation is returned to the pool when the last [`PooledBytes`]
    /// reference is dropped.
    pub fn freeze(self) -> PooledBytes<'a> {
        PooledBytes {
            pool: self.pool,
            bytes: Bytes::from(self.buf),
        }
    }
}

/// An immutable, reference-counted byte buffer tied to a [`BufferPool`].
///
/// Cheap to clone: all clones share the same underlying allocation.  When the
/// final clone is dropped and no other [`Bytes`] references exist, the buffer
/// is cleared and returned to the pool.
#[derive(Debug, Clone)]
pub struct PooledBytes<'a> {
    pool: &'a BufferPool,
    bytes: Bytes,
}

impl<'a> PooledBytes<'a> {
    /// Number of bytes in this buffer.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether this buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Extract the underlying [`Bytes`], abandoning the pool.
    ///
    /// The allocation is freed normally (not returned to the pool) once the
    /// returned [`Bytes`] is dropped.  This is the escape hatch for bytes that
    /// must outlive the pool or that cannot be recycled.
    pub fn into_inner(self) -> Bytes {
        // `PooledBytes` implements `Drop`, so we cannot move the field out
        // directly.  Wrapping in `ManuallyDrop` suppresses the destructor (the
        // buffer must NOT go back to the pool — the caller is abandoning it),
        // and `mem::take` moves the `Bytes` out without leaving the wrapper
        // partially initialized.
        let mut this = std::mem::ManuallyDrop::new(self);
        std::mem::take(&mut this.bytes)
    }
}

impl AsRef<[u8]> for PooledBytes<'_> {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for PooledBytes<'_> {
    fn drop(&mut self) {
        // `Bytes::try_into_mut` only succeeds when we are the unique owner of
        // the underlying allocation (refcount == 1), i.e. no clones or derived
        // `Bytes` still exist.  Otherwise the buffer is still shared and must
        // not be recycled; the returned `Bytes` is dropped here, decrementing
        // the refcount so the final owner can recycle it.
        if let Ok(buf) = std::mem::take(&mut self.bytes).try_into_mut() {
            // `try_into_mut` yields a `BytesMut`; converting back to `Vec<u8>`
            // is zero-copy for buffers that came from a `Vec` (which ours
            // always do), so the original allocation is recycled as-is.
            self.pool.release(buf.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn acquire_freeze_drop_returns_buffer() {
        let pool = BufferPool::new(2, 64);
        // First acquire allocates fresh.
        let mut buf = pool.acquire();
        buf.inner_mut().extend_from_slice(b"hello world");
        let frozen = buf.freeze();
        assert_eq!(frozen.as_ref(), b"hello world");
        assert_eq!(frozen.len(), 11);
        assert!(!frozen.is_empty());
        // Nothing is pooled while the frozen buffer is alive.
        assert_eq!(pool.buffers().len(), 0);

        drop(frozen);
        // The buffer was cleared and returned to the pool.
        let returned = pool.buffers();
        assert_eq!(returned.len(), 1);
        assert!(returned[0].is_empty());
        assert!(returned[0].capacity() >= 64);
        drop(returned);

        // The next acquire reuses the pooled buffer (retained capacity).
        let mut buf2 = pool.acquire();
        assert!(buf2.inner_mut().capacity() >= 64);
        assert!(buf2.inner_mut().is_empty());
    }

    #[test]
    fn pool_exhaustion_falls_back_to_heap() {
        let pool = BufferPool::new(1, 16);
        // More live buffers than the pool can hold: all succeed, none panic.
        let mut b1 = pool.acquire();
        let mut b2 = pool.acquire();
        let mut b3 = pool.acquire();
        b1.inner_mut().extend_from_slice(b"one");
        b2.inner_mut().extend_from_slice(b"two");
        b3.inner_mut().extend_from_slice(b"three");

        let f1 = b1.freeze();
        let f2 = b2.freeze();
        let f3 = b3.freeze();
        assert_eq!(f1.as_ref(), b"one");
        assert_eq!(f2.as_ref(), b"two");
        assert_eq!(f3.as_ref(), b"three");

        drop(f1);
        drop(f2);
        drop(f3);
        // The pool retains at most `pool_size` buffers.
        assert_eq!(pool.buffers().len(), 1);
    }

    #[test]
    fn cloned_bytes_share_allocation_and_return_once() {
        let pool = BufferPool::new(1, 64);
        let mut buf = pool.acquire();
        buf.inner_mut().extend_from_slice(b"shared");
        let original = buf.freeze();

        let clone = original.clone();
        assert_eq!(clone.as_ref(), b"shared");
        assert_eq!(original.as_ref(), b"shared");

        // Dropping the original while the clone is alive must NOT recycle the
        // buffer: the allocation is still shared.
        drop(original);
        assert_eq!(pool.buffers().len(), 0);

        // Dropping the last reference returns the buffer to the pool.
        drop(clone);
        assert_eq!(pool.buffers().len(), 1);
        assert!(pool.buffers()[0].is_empty());
    }

    #[test]
    fn into_inner_abandons_pool() {
        let pool = BufferPool::new(1, 64);
        let mut buf = pool.acquire();
        buf.inner_mut().extend_from_slice(b"escape");
        let pooled = buf.freeze();
        let raw: Bytes = pooled.into_inner();
        assert_eq!(&raw[..], b"escape");
        drop(raw);
        // The buffer was handed off, not returned to the pool.
        assert_eq!(pool.buffers().len(), 0);
    }

    #[test]
    fn dropping_unfrozen_buffer_is_harmless() {
        let pool = BufferPool::new(2, 64);
        {
            let mut buf = pool.acquire();
            buf.inner_mut().extend_from_slice(b"discarded");
        } // dropped without freeze: Vec freed, pool unchanged
        assert_eq!(pool.buffers().len(), 0);

        // The pool still works afterwards.
        let mut buf = pool.acquire();
        buf.inner_mut().extend_from_slice(b"still works");
        assert_eq!(buf.freeze().as_ref(), b"still works");
    }

    #[test]
    fn concurrent_acquire_freeze_drop_is_safe() {
        let pool = Arc::new(BufferPool::new(4, 128));
        let mut handles = Vec::new();
        for i in 0..8 {
            let pool = Arc::clone(&pool);
            handles.push(thread::spawn(move || {
                for round in 0..100 {
                    let mut buf = pool.acquire();
                    let payload = format!("{i}-{round}");
                    buf.inner_mut().extend_from_slice(payload.as_bytes());
                    let frozen = buf.freeze();
                    assert_eq!(frozen.as_ref(), payload.as_bytes());
                    drop(frozen);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        // All threads finished; the pool never grew past its bound.
        assert!(pool.buffers().len() <= 4);
    }
}
