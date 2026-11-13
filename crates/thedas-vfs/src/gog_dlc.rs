//! Offline lookup in DAOrigins.exe from the GOG Ultimate Edition.
//!
//! Observed in SHA-256 2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3.
//! The jump table at RVA 0x884f8 is indexed by module ID minus 100. A case starts with a
//! push of the encoded string's address. At RVA 0x8843a the game maps each character
//! to a digit with (character - 'A') / 2. In this PE the relevant RVAs equal file offsets.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::crypto::DaoKey;

const IMAGE_BASE: u32 = 0x400000;
const TABLE: u32 = 0x884f8;

pub(crate) fn key(archive: &Path, module: u32, digest: &[u8; 16]) -> Option<DaoKey> {
    let index = module.checked_sub(100).filter(|i| *i <= 50)?;
    let mut exe = archive
        .parent()?
        .ancestors()
        .find_map(|dir| File::open(dir.join("bin_ship/DAOrigins.exe")).ok())?;

    let case = u32::from_le_bytes(read_at(&mut exe, TABLE + 4 * index)?).checked_sub(IMAGE_BASE)?;
    // This interval contains the offline cases. The entitlement fallback lies after it.
    if !(0x878c0..=0x88346).contains(&case) {
        return None;
    }
    let [push, address @ ..] = read_at::<5>(&mut exe, case)?;
    if push != 0x68 {
        return None;
    }
    let offset = u32::from_le_bytes(address).checked_sub(IMAGE_BASE)?;
    if !(0x6eb0e0..=0x6eb320).contains(&offset) {
        return None;
    }
    let [encoded @ .., terminator] = read_at::<9>(&mut exe, offset)?;
    if terminator != 0 {
        return None;
    }
    // A file with a different layout can only supply a key if the archive confirms it.
    let key = decode(encoded)?;
    key.matches_digest(digest).then_some(key)
}

fn decode(encoded: [u8; 8]) -> Option<DaoKey> {
    let mut digits = encoded;
    for byte in &mut digits {
        if !(b'A'..=b'T').contains(byte) {
            return None;
        }
        *byte = b'0' + (*byte - b'A') / 2;
    }
    Some(DaoKey::from_bytes(digits))
}

fn read_at<const N: usize>(exe: &mut File, offset: u32) -> Option<[u8; N]> {
    let mut bytes = [0; N];
    exe.seek(SeekFrom::Start(offset.into())).ok()?;
    exe.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use md5::{Digest, Md5};

    #[test]
    fn decodes_digit_pairs_without_losing_leading_zeroes() {
        let key = decode(*b"ABCDSTEF").unwrap();
        assert!(key.matches_digest(&Md5::digest(b"00119922").into()));
        assert_eq!(key.blowfish_key(), Some(119922u64.to_le_bytes()));
        assert!(decode(*b"ABCDEFGU").is_none());
    }
}
