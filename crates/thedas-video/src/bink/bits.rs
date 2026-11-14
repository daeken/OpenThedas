//! Bink's bit reader: bits come least significant first, out of little-endian bytes.

/// Reads bits from a packet. Reading past the end gives zeros and marks the reader as
/// overrun, so a damaged packet ends in an error instead of a panic.
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    /// Position in bits.
    pos: usize,
    overrun: bool,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            overrun: false,
        }
    }

    /// The next 64 bits from `pos` on (zeros past the end), lowest first.
    #[inline]
    fn window(&self) -> u64 {
        let byte = self.pos >> 3;
        let shift = self.pos & 7;
        let w = match self.data.get(byte..byte + 8) {
            Some(b) => u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            None => {
                let mut b = [0u8; 8];
                if let Some(rest) = self.data.get(byte..) {
                    b[..rest.len()].copy_from_slice(rest);
                }
                u64::from_le_bytes(b)
            }
        };
        w >> shift
    }

    /// The next `n` (at most 32) bits, without taking them.
    #[inline]
    pub(crate) fn peek(&self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        (self.window() & ((1u64 << n) - 1)) as u32
    }

    #[inline]
    pub(crate) fn skip(&mut self, n: u32) {
        self.pos += n as usize;
        if self.pos > self.data.len() * 8 {
            self.overrun = true;
        }
    }

    /// The next `n` (at most 32) bits, as a number.
    #[inline]
    pub(crate) fn read(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.skip(n);
        v
    }

    #[inline]
    pub(crate) fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    /// Bits left to read (negative once overrun).
    pub(crate) fn left(&self) -> isize {
        self.data.len() as isize * 8 - self.pos as isize
    }

    /// Moves on to the next multiple of 32 bits.
    pub(crate) fn align32(&mut self) {
        let r = self.pos & 31;
        if r != 0 {
            self.skip(32 - r as u32);
        }
    }

    pub(crate) fn overrun(&self) -> bool {
        self.overrun
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_come_lowest_first_across_bytes() {
        let mut b = Bits::new(&[0b1010_1100, 0xff, 0x01]);
        assert_eq!(b.read(2), 0b00);
        assert_eq!(b.read(3), 0b011);
        assert_eq!(b.read(5), 0b11_101);
        assert_eq!(b.read(14), 0b111_1111);
        assert_eq!(b.left(), 0);
        assert!(!b.overrun());
        assert_eq!(b.read(3), 0);
        assert!(b.overrun());
    }

    #[test]
    fn align32_skips_to_the_next_word() {
        let mut b = Bits::new(&[0; 12]);
        b.skip(1);
        b.align32();
        assert_eq!(b.left(), 64);
        b.align32();
        assert_eq!(b.left(), 64);
    }
}
