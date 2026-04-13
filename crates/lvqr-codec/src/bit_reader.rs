//! Forward, MSB-first bit reader with exp-Golomb decoders.
//!
//! Used by every codec parser in this crate. Behavior:
//!
//! * Bits are consumed MSB first within each byte.
//! * `read_bits(n)` supports `1 <= n <= 32`. Larger widths are not
//!   meaningful for any codec field in scope.
//! * `read_ue_v` / `read_se_v` implement H.264/H.265 unsigned and signed
//!   exp-Golomb coding. Codes wider than 32 bits return
//!   [`CodecError::GolombOverflow`] rather than panicking.
//! * End-of-stream always returns a structured [`CodecError::EndOfStream`];
//!   no panics on arbitrary input. This is the invariant the proptest
//!   harness relies on.
//!
//! Emulation-prevention byte (0x03) removal lives on
//! [`rbsp_from_ebsp`] because it is codec-agnostic.

use crate::error::CodecError;

pub struct BitReader<'a> {
    bytes: &'a [u8],
    /// Bit cursor in the byte stream, counted from the MSB of byte 0.
    /// At position `p`, the next bit returned is bit `7 - (p % 8)` of
    /// `bytes[p / 8]`.
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    #[inline]
    pub fn bits_read(&self) -> usize {
        self.pos
    }

    #[inline]
    pub fn bits_remaining(&self) -> usize {
        self.bytes.len().saturating_mul(8).saturating_sub(self.pos)
    }

    pub fn read_bit(&mut self) -> Result<u8, CodecError> {
        if self.bits_remaining() == 0 {
            return Err(CodecError::EndOfStream {
                needed: 1,
                remaining: 0,
            });
        }
        let byte = self.bytes[self.pos / 8];
        let shift = 7 - (self.pos % 8);
        self.pos += 1;
        Ok((byte >> shift) & 1)
    }

    /// Read up to 32 bits. Panics are impossible: oversize requests return
    /// [`CodecError::GolombOverflow`], underruns return
    /// [`CodecError::EndOfStream`].
    pub fn read_bits(&mut self, n: u8) -> Result<u32, CodecError> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 {
            return Err(CodecError::GolombOverflow);
        }
        if self.bits_remaining() < n as usize {
            return Err(CodecError::EndOfStream {
                needed: n as usize,
                remaining: self.bits_remaining(),
            });
        }
        let mut value: u32 = 0;
        for _ in 0..n {
            // Inline read_bit body to avoid re-checking remaining bits per bit.
            let byte = self.bytes[self.pos / 8];
            let shift = 7 - (self.pos % 8);
            self.pos += 1;
            value = (value << 1) | ((byte >> shift) as u32 & 1);
        }
        Ok(value)
    }

    pub fn skip_bits(&mut self, n: usize) -> Result<(), CodecError> {
        if self.bits_remaining() < n {
            return Err(CodecError::EndOfStream {
                needed: n,
                remaining: self.bits_remaining(),
            });
        }
        self.pos += n;
        Ok(())
    }

    /// Exp-Golomb unsigned (`ue(v)` in the H.26x specs).
    ///
    /// Encoding: `k` leading zero bits, then a 1 bit, then `k` value
    /// bits. The decoded value is `(1 << k) - 1 + suffix`. Caps the leading
    /// zero count at 32 since any code with 33+ leading zeros decodes to a
    /// value that does not fit in u32.
    pub fn read_ue_v(&mut self) -> Result<u32, CodecError> {
        let mut leading_zeros: u32 = 0;
        while self.read_bit()? == 0 {
            leading_zeros += 1;
            if leading_zeros > 32 {
                return Err(CodecError::GolombOverflow);
            }
        }
        if leading_zeros == 0 {
            return Ok(0);
        }
        let suffix = self.read_bits(leading_zeros as u8)?;
        // (1 << leading_zeros) - 1 + suffix. Compute in u64 to avoid
        // overflow when leading_zeros == 32.
        let base: u64 = (1u64 << leading_zeros) - 1;
        let total = base + suffix as u64;
        if total > u32::MAX as u64 {
            return Err(CodecError::GolombOverflow);
        }
        Ok(total as u32)
    }

    /// Exp-Golomb signed (`se(v)` in the H.26x specs).
    ///
    /// Decoded as `ue(v)` then mapped: 0 -> 0, 1 -> 1, 2 -> -1, 3 -> 2,
    /// 4 -> -2, ...
    pub fn read_se_v(&mut self) -> Result<i32, CodecError> {
        let code = self.read_ue_v()?;
        if code == 0 {
            return Ok(0);
        }
        let magnitude = (code / 2 + code % 2) as i64;
        if code & 1 == 1 {
            // odd codes are positive
            Ok(magnitude as i32)
        } else {
            // even codes are negative
            Ok(-(magnitude as i32))
        }
    }
}

/// Strip H.264/H.265 emulation-prevention bytes: whenever the encoder
/// emits `0x00 0x00 0x00` or `0x00 0x00 0x01` or `0x00 0x00 0x02` or
/// `0x00 0x00 0x03` in the NAL payload, it inserts a `0x03` byte after
/// the two zeros to distinguish from a start code. Reverse that
/// transformation so the decoder sees the raw RBSP.
pub fn rbsp_from_ebsp(ebsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ebsp.len());
    let mut i = 0;
    while i < ebsp.len() {
        if i + 2 < ebsp.len() && ebsp[i] == 0x00 && ebsp[i + 1] == 0x00 && ebsp[i + 2] == 0x03 {
            out.push(0x00);
            out.push(0x00);
            i += 3;
        } else {
            out.push(ebsp[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_single_bits() {
        let mut r = BitReader::new(&[0b1010_1100]);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert!(matches!(r.read_bit(), Err(CodecError::EndOfStream { .. })));
    }

    #[test]
    fn read_multi_bits() {
        // 0xAC = 10101100, then 0x0F = 00001111
        let mut r = BitReader::new(&[0xAC, 0x0F]);
        assert_eq!(r.read_bits(4).unwrap(), 0b1010);
        assert_eq!(r.read_bits(4).unwrap(), 0b1100);
        assert_eq!(r.read_bits(8).unwrap(), 0x0F);
    }

    #[test]
    fn read_bits_spanning_byte_boundary() {
        // Read 12 bits of 0xABCD = 101010111100_1101 -> first 12 bits = 0xABC
        let mut r = BitReader::new(&[0xAB, 0xCD]);
        assert_eq!(r.read_bits(12).unwrap(), 0xABC);
        assert_eq!(r.read_bits(4).unwrap(), 0xD);
    }

    #[test]
    fn ue_v_decodes_known_values() {
        // 0  -> "1"
        // 1  -> "010"
        // 2  -> "011"
        // 3  -> "00100"
        // 4  -> "00101"
        // 7  -> "0001000"
        // Construct a byte with bits: 1 010 011 00100 0 (padding) = 16 bits
        // 1 010 011 00100 0
        // = 1010 0110 0100 0000 = 0xA6 0x40
        let mut r = BitReader::new(&[0xA6, 0x40]);
        assert_eq!(r.read_ue_v().unwrap(), 0);
        assert_eq!(r.read_ue_v().unwrap(), 1);
        assert_eq!(r.read_ue_v().unwrap(), 2);
        assert_eq!(r.read_ue_v().unwrap(), 3);
    }

    #[test]
    fn se_v_mapping() {
        // code 0 -> 0, code 1 -> 1, code 2 -> -1, code 3 -> 2, code 4 -> -2
        // Encode: 0, 1, -1, 2, -2 as se(v)
        // ue codes: 0(1), 1(010), 2(011), 3(00100), 4(00101)
        // stream: 1 010 011 00100 00101 -> 1010 0110 0100 0010 1
        //       = 0xA6 0x42 0x80
        let mut r = BitReader::new(&[0xA6, 0x42, 0x80]);
        assert_eq!(r.read_se_v().unwrap(), 0);
        assert_eq!(r.read_se_v().unwrap(), 1);
        assert_eq!(r.read_se_v().unwrap(), -1);
        assert_eq!(r.read_se_v().unwrap(), 2);
        assert_eq!(r.read_se_v().unwrap(), -2);
    }

    #[test]
    fn rbsp_strips_emulation_byte() {
        // 00 00 03 01 -> 00 00 01
        assert_eq!(rbsp_from_ebsp(&[0x00, 0x00, 0x03, 0x01]), vec![0x00, 0x00, 0x01]);
        // unaffected
        assert_eq!(rbsp_from_ebsp(&[0x01, 0x02, 0x03]), vec![0x01, 0x02, 0x03]);
        // two successive strippings
        assert_eq!(
            rbsp_from_ebsp(&[0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0xFF]),
            vec![0x00, 0x00, 0x00, 0x00, 0xFF]
        );
    }

    #[test]
    fn ue_v_overflow_guard() {
        // 33 leading zeros would overflow u32. Test with a pathological input.
        let bytes = [0u8; 8]; // 64 zero bits, no terminating 1
        let mut r = BitReader::new(&bytes);
        assert!(matches!(r.read_ue_v(), Err(CodecError::GolombOverflow)));
    }
}
