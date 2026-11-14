//! A bounds-checked little-endian reader with SWF's bit fields.

use super::SwfError;

pub type Result<T> = std::result::Result<T, SwfError>;

#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u8,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bit: 0,
        }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub fn rest(&self) -> &'a [u8] {
        self.data.get(self.pos..).unwrap_or(&[])
    }

    pub fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.pos += 1;
        }
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.align();
        let end = self.pos.checked_add(n).ok_or(SwfError::Truncated)?;
        let s = self.data.get(self.pos..end).ok_or(SwfError::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    /// AVM1's double: two little-endian 32-bit halves, high half first.
    pub fn f64_swapped(&mut self) -> Result<f64> {
        let hi = self.u32()? as u64;
        let lo = self.u32()? as u64;
        Ok(f64::from_bits((hi << 32) | lo))
    }

    /// 16.16 fixed point.
    pub fn fixed(&mut self) -> Result<f32> {
        Ok(self.i32()? as f32 / 65536.0)
    }

    /// 8.8 fixed point.
    pub fn fixed8(&mut self) -> Result<f32> {
        Ok(self.i16()? as f32 / 256.0)
    }

    /// A NUL-terminated string (SWF 6+ text is UTF-8; invalid bytes are replaced).
    pub fn cstr(&mut self) -> Result<String> {
        self.align();
        let rest = self.rest();
        let n = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(SwfError::Truncated)?;
        let s = String::from_utf8_lossy(&rest[..n]).into_owned();
        self.pos += n + 1;
        Ok(s)
    }

    /// A string with a one-byte length.
    pub fn pstr(&mut self) -> Result<String> {
        let n = self.u8()? as usize;
        let b = self.bytes(n)?;
        Ok(String::from_utf8_lossy(b)
            .trim_end_matches('\0')
            .to_string())
    }

    pub fn ub(&mut self, n: u32) -> Result<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.data.get(self.pos).ok_or(SwfError::Truncated)?;
            v = (v << 1) | u32::from((byte >> (7 - self.bit)) & 1);
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        Ok(v)
    }

    pub fn sb(&mut self, n: u32) -> Result<i32> {
        let v = self.ub(n)?;
        if n > 0 && n < 32 && v & (1 << (n - 1)) != 0 {
            Ok((v | !((1u32 << n) - 1)) as i32)
        } else {
            Ok(v as i32)
        }
    }

    /// A signed 16.16 fixed-point bit field.
    pub fn fb(&mut self, n: u32) -> Result<f32> {
        Ok(self.sb(n)? as f32 / 65536.0)
    }

    pub fn flag(&mut self) -> Result<bool> {
        Ok(self.ub(1)? != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_and_bounds() {
        let mut r = Reader::new(&[0b1010_0000, 0xff]);
        assert_eq!(r.ub(3).unwrap(), 0b101);
        assert_eq!(r.sb(2).unwrap(), 0);
        r.align();
        assert_eq!(r.sb(4).unwrap(), -1);
        assert!(r.u16().is_err());
        assert!(Reader::new(&[1, 2]).cstr().is_err());
    }
}
