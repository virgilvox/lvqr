use std::io::Cursor;

/// A contiguous buffer of bytes.
// We're not using bytes::Buf because of some strange bugs with take().
pub trait Buf {
    fn remaining(&self) -> usize;
    fn has_remaining(&self) -> bool {
        self.remaining() > 0
    }

    fn slice(&self, size: usize) -> &[u8];
    fn advance(&mut self, n: usize);
}

impl Buf for &[u8] {
    fn remaining(&self) -> usize {
        self.len()
    }

    // LVQR hardening: clamp to the available bytes so an adversarial
    // (truncated) atom body cannot panic the decoder on an
    // out-of-bounds slice/advance. For valid input there is always
    // enough, so this is a no-op; for malformed input the decoder gets
    // a short/empty slice and surfaces a decode error instead of a
    // process-aborting panic (fuzz-found via subs.rs).
    fn slice(&self, size: usize) -> &[u8] {
        &self[..size.min(self.len())]
    }

    fn advance(&mut self, n: usize) {
        *self = &self[n.min(self.len())..];
    }
}

impl<T: AsRef<[u8]>> Buf for Cursor<T> {
    fn remaining(&self) -> usize {
        // saturating: a position past the end (after a clamped advance)
        // must not underflow into a huge `remaining`.
        let len = self.get_ref().as_ref().len();
        len.saturating_sub(self.position() as usize)
    }

    // LVQR hardening: clamp to the available bytes (see the &[u8] impl).
    fn slice(&self, size: usize) -> &[u8] {
        let data = self.get_ref().as_ref();
        let len = data.len();
        let start = (self.position() as usize).min(len);
        let end = start.saturating_add(size).min(len);
        &data[start..end]
    }

    fn advance(&mut self, n: usize) {
        self.set_position(self.position() + n as u64);
    }
}

impl<T: Buf + ?Sized> Buf for &mut T {
    fn remaining(&self) -> usize {
        (**self).remaining()
    }

    fn slice(&self, size: usize) -> &[u8] {
        (**self).slice(size)
    }

    fn advance(&mut self, n: usize) {
        (**self).advance(n);
    }
}

#[cfg(feature = "bytes")]
impl Buf for bytes::Bytes {
    fn remaining(&self) -> usize {
        self.len()
    }

    // LVQR hardening: clamp to the available bytes (see the &[u8] impl).
    fn slice(&self, size: usize) -> &[u8] {
        &self[..size.min(self.len())]
    }

    fn advance(&mut self, n: usize) {
        bytes::Buf::advance(self, n.min(self.len()));
    }
}

/// A mutable contiguous buffer of bytes.
// We're not using bytes::BufMut because it doesn't allow seeking backwards (to set the size).
pub trait BufMut {
    // Returns the current length.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // Append a slice to the buffer
    fn append_slice(&mut self, val: &[u8]);

    // Set a slice at a position in the buffer.
    fn set_slice(&mut self, pos: usize, val: &[u8]);
}

impl BufMut for Vec<u8> {
    fn len(&self) -> usize {
        self.len()
    }

    fn append_slice(&mut self, v: &[u8]) {
        self.extend_from_slice(v);
    }

    fn set_slice(&mut self, pos: usize, val: &[u8]) {
        self[pos..pos + val.len()].copy_from_slice(val);
    }
}

impl<T: BufMut + ?Sized> BufMut for &mut T {
    fn len(&self) -> usize {
        (**self).len()
    }

    fn append_slice(&mut self, v: &[u8]) {
        (**self).append_slice(v);
    }

    fn set_slice(&mut self, pos: usize, val: &[u8]) {
        (**self).set_slice(pos, val);
    }
}

#[cfg(feature = "bytes")]
impl BufMut for bytes::BytesMut {
    fn len(&self) -> usize {
        self.len()
    }

    fn append_slice(&mut self, v: &[u8]) {
        self.extend_from_slice(v);
    }

    fn set_slice(&mut self, pos: usize, val: &[u8]) {
        self[pos..pos + val.len()].copy_from_slice(val);
    }
}
