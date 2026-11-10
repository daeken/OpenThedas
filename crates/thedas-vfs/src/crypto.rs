//! Encryption used by Dragon Age: Origins DLC archives (ERF V2.2, encryption type 2).
//!
//! Each archive stores the MD5 digest of a decimal password string. The Blowfish key is that
//! number as 8 little-endian bytes, and resources are encrypted with Blowfish in ECB mode.
//! Where the original game gets the password from is still an open question: it is not
//! stored in the archive itself.

use blowfish::Blowfish;
use blowfish::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};
use md5::{Digest, Md5};

/// A DLC key as the original game handles it: the decimal password string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaoKey {
    password: String,
}

impl DaoKey {
    pub fn new(password: impl Into<String>) -> Self {
        Self {
            password: password.into(),
        }
    }

    /// Whether this password matches an archive's stored digest.
    pub fn matches_digest(&self, digest: &[u8; 16]) -> bool {
        Md5::digest(self.password.as_bytes()).as_slice() == digest
    }

    /// The Blowfish key bytes: the password parsed as an integer, as 8 little-endian bytes.
    /// Returns `None` if the password is not a decimal number.
    pub fn blowfish_key(&self) -> Option<[u8; 8]> {
        self.password
            .trim()
            .parse::<u64>()
            .ok()
            .map(u64::to_le_bytes)
    }
}

/// Decrypts `data` in place with Blowfish ECB. A trailing partial block is zero-padded,
/// decrypted, and truncated back to its original length.
pub fn blowfish_ecb_decrypt(key: &[u8], data: &mut [u8]) {
    let cipher: Blowfish =
        Blowfish::new_from_slice(key).expect("Blowfish accepts 4 to 56 byte keys");
    let mut chunks = data.chunks_exact_mut(8);
    for block in &mut chunks {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
    }
    let rest = chunks.into_remainder();
    if !rest.is_empty() {
        let mut block = [0u8; 8];
        block[..rest.len()].copy_from_slice(rest);
        cipher.decrypt_block(GenericArray::from_mut_slice(&mut block));
        let len = rest.len();
        rest.copy_from_slice(&block[..len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blowfish::cipher::BlockEncrypt;

    #[test]
    fn key_is_little_endian_number() {
        let key = DaoKey::new("258");
        assert_eq!(key.blowfish_key(), Some([2, 1, 0, 0, 0, 0, 0, 0]));
        assert_eq!(DaoKey::new("not a number").blowfish_key(), None);
    }

    #[test]
    fn digest_is_md5_of_password() {
        let key = DaoKey::new("12345");
        let digest: [u8; 16] = Md5::digest(b"12345").into();
        assert!(key.matches_digest(&digest));
        assert!(!DaoKey::new("12346").matches_digest(&digest));
    }

    #[test]
    fn ecb_round_trip() {
        let key = DaoKey::new("1234567890").blowfish_key().unwrap();
        let plain: Vec<u8> = (0u8..32).collect();
        let mut data = plain.clone();
        let cipher: Blowfish = Blowfish::new_from_slice(&key).unwrap();
        for block in data.chunks_exact_mut(8) {
            cipher.encrypt_block(GenericArray::from_mut_slice(block));
        }
        assert_ne!(data, plain);
        blowfish_ecb_decrypt(&key, &mut data);
        assert_eq!(data, plain);
    }
}
