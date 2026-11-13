//! The ERF archive family used by Dragon Age.
//!
//! | Version | Magic encoding | Used by | Entry table |
//! | --- | --- | --- | --- |
//! | V2.0 | UTF-16LE | DA:O PC `.erf` and `.rim` | 32-char UTF-16 name, offset, size |
//! | V2.1 | ASCII | a few DA:O DLC `.rim` files | 32-char ASCII name, offset, packed, size; zlib |
//! | V2.2 | UTF-16LE | DA:O DLC | 32-char UTF-16 name, offset, packed, size; flags, digest |
//! | V3.0 | UTF-16LE | Dragon Age II | name string table, FNV hashes, offset, packed, size |
//!
//! Layouts were cross-checked against xoreos and the files in the DA:O GOG install.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::crypto::{DaoKey, blowfish_ecb_decrypt, decrypt_blocks};
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErfVersion {
    V20,
    V21,
    V22,
    V30,
}

impl ErfVersion {
    pub fn as_str(self) -> &'static str {
        match self {
            ErfVersion::V20 => "V2.0",
            ErfVersion::V21 => "V2.1",
            ErfVersion::V22 => "V2.2",
            ErfVersion::V30 => "V3.0",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encryption {
    None,
    Xor,
    BlowfishDao,
    BlowfishDa2,
    Unknown(u32),
}

impl Encryption {
    fn from_flags(flags: u32) -> Self {
        match (flags >> 4) & 0xF {
            0 => Encryption::None,
            1 => Encryption::Xor,
            2 => Encryption::BlowfishDao,
            3 => Encryption::BlowfishDa2,
            n => Encryption::Unknown(n),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    /// Raw DEFLATE preceded by one byte whose high nibble is the window size.
    BiowareZlib,
    Lzma,
    XboxLzx,
    /// Raw DEFLATE with no header.
    HeaderlessZlib,
    /// A standard zlib stream (RFC 1950).
    StandardZlib,
    Unknown(u32),
}

impl Compression {
    fn from_flags(flags: u32) -> Self {
        Self::from_code((flags >> 29) & 0x7)
    }

    fn from_code(code: u32) -> Self {
        match code {
            0 => Compression::None,
            1 => Compression::BiowareZlib,
            2 => Compression::Lzma,
            3 => Compression::XboxLzx,
            7 => Compression::HeaderlessZlib,
            8 => Compression::StandardZlib,
            n => Compression::Unknown(n),
        }
    }

    fn code(self) -> u32 {
        match self {
            Compression::None => 0,
            Compression::BiowareZlib => 1,
            Compression::Lzma => 2,
            Compression::XboxLzx => 3,
            Compression::HeaderlessZlib => 7,
            Compression::StandardZlib => 8,
            Compression::Unknown(n) => n,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ErfEntry {
    /// The resource name with extension, as stored. V3.0 entries may have no name, only a hash.
    pub name: Option<String>,
    /// V3.0 only: FNV-64 hash of the name.
    pub name_hash: Option<u64>,
    /// V3.0 only: FNV-32 hash of the type.
    pub type_hash: Option<u32>,
    pub offset: u32,
    pub packed_size: u32,
    pub size: u32,
}

impl ErfEntry {
    /// A printable name: the stored name, or the hash for unnamed V3.0 entries.
    pub fn display_name(&self) -> String {
        match (&self.name, self.name_hash) {
            (Some(n), _) => n.clone(),
            (None, Some(h)) => format!("#{h:016x}"),
            (None, None) => "<unnamed>".to_string(),
        }
    }
}

/// An open ERF archive. Reads go through a shared file handle, so an `Erf` can be shared
/// between threads behind an `Arc`.
#[derive(Debug)]
pub struct Erf {
    path: PathBuf,
    file: Mutex<File>,
    installed_key: OnceLock<Option<DaoKey>>,
    pub version: ErfVersion,
    pub encryption: Encryption,
    pub compression: Compression,
    pub module_id: u32,
    pub password_digest: Option<[u8; 16]>,
    pub build_year: u32,
    pub build_day: u32,
    pub entries: Vec<ErfEntry>,
}

/// Reads the version of an ERF from its first 16 bytes, if it is one.
pub fn sniff_version(header: &[u8]) -> Option<ErfVersion> {
    if header.len() < 16 {
        return None;
    }
    let version_of = |v: &str| match v {
        "V2.0" => Some(ErfVersion::V20),
        "V2.1" => Some(ErfVersion::V21),
        "V2.2" => Some(ErfVersion::V22),
        "V3.0" => Some(ErfVersion::V30),
        _ => None,
    };
    if &header[..4] == b"ERF " {
        return std::str::from_utf8(&header[4..8]).ok().and_then(version_of);
    }
    let utf16: Vec<u16> = header[..16]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let magic = String::from_utf16(&utf16).ok()?;
    magic.strip_prefix("ERF ").and_then(version_of)
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    path: &'a Path,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8], pos: usize, path: &'a Path) -> Self {
        Self { data, pos, path }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        match end {
            Some(end) => {
                let s = &self.data[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            None => Err(malformed(
                self.path,
                "header or entry table runs past end of file",
            )),
        }
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn utf16_fixed(&mut self, bytes: usize) -> Result<String> {
        let raw = self.take(bytes)?;
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16(&units)
            .map_err(|_| malformed(self.path, "entry name is not valid UTF-16"))
    }

    fn ascii_fixed(&mut self, bytes: usize) -> Result<String> {
        let raw = self.take(bytes)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Ok(raw[..end].iter().map(|&b| b as char).collect())
    }
}

fn malformed(path: &Path, reason: impl Into<String>) -> Error {
    Error::Malformed {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

fn io_err(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

impl Erf {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let mut file = File::open(path).map_err(|e| io_err(path, e))?;
        let file_len = file.metadata().map_err(|e| io_err(path, e))?.len();

        let mut head = [0u8; 64];
        let head_len = read_up_to(&mut file, &mut head).map_err(|e| io_err(path, e))?;
        let version = sniff_version(&head[..head_len]).ok_or_else(|| match head_len >= 16 {
            true if head.starts_with(b"ERF ") || head.starts_with(b"E\0R\0F\0 \0") => {
                Error::UnsupportedVersion {
                    path: path.to_path_buf(),
                    version: String::from_utf8_lossy(&head[..16]).replace('\0', ""),
                }
            }
            _ => Error::NotErf {
                path: path.to_path_buf(),
            },
        })?;

        // Read the fixed header plus the entry table in one go.
        let table_bytes = |count: u64, fixed: u64, entry: u64| fixed + count * entry;
        let count = u32::from_le_bytes(head[16..20].try_into().unwrap()) as u64;
        let header_and_table = match version {
            ErfVersion::V20 => table_bytes(count, 32, 72),
            ErfVersion::V21 => table_bytes(count, 32, 44),
            ErfVersion::V22 => table_bytes(count, 56, 76),
            ErfVersion::V30 => {
                // [16] string table size, [20] resource count.
                let strings = count; // field at 16 is the string table size in V3.0
                let res_count = u32::from_le_bytes(head[20..24].try_into().unwrap()) as u64;
                48 + strings + res_count * 28
            }
        };
        if header_and_table > file_len {
            return Err(malformed(path, "entry table runs past end of file"));
        }
        let mut buf = vec![0u8; header_and_table as usize];
        file.seek(SeekFrom::Start(0)).map_err(|e| io_err(path, e))?;
        file.read_exact(&mut buf).map_err(|e| io_err(path, e))?;

        let mut c = Cursor::new(&buf, 16, path);
        let mut erf = Erf {
            path: path.to_path_buf(),
            file: Mutex::new(file),
            installed_key: OnceLock::new(),
            version,
            encryption: Encryption::None,
            compression: Compression::None,
            module_id: 0,
            password_digest: None,
            build_year: 0,
            build_day: 0,
            entries: Vec::with_capacity(count.min(1 << 20) as usize),
        };

        match version {
            ErfVersion::V20 => {
                let count = c.u32()?;
                erf.build_year = c.u32()? + 1900;
                erf.build_day = c.u32()?;
                c.u32()?; // Unknown, 0xFFFFFFFF in every file seen.
                for _ in 0..count {
                    let name = c.utf16_fixed(64)?;
                    let offset = c.u32()?;
                    let size = c.u32()?;
                    erf.entries.push(ErfEntry {
                        name: Some(name),
                        name_hash: None,
                        type_hash: None,
                        offset,
                        packed_size: size,
                        size,
                    });
                }
            }
            ErfVersion::V21 => {
                let count = c.u32()?;
                c.u32()?; // Unknown, zero.
                erf.build_year = c.u16()? as u32 + 1900;
                erf.build_day = c.u16()? as u32;
                c.u32()?; // Unknown, 0x0000FFFF.
                erf.compression = Compression::StandardZlib;
                for _ in 0..count {
                    let name = c.ascii_fixed(32)?;
                    let offset = c.u32()?;
                    let packed_size = c.u32()?;
                    let size = c.u32()?;
                    erf.entries.push(ErfEntry {
                        name: Some(name),
                        name_hash: None,
                        type_hash: None,
                        offset,
                        packed_size,
                        size,
                    });
                }
            }
            ErfVersion::V22 => {
                let count = c.u32()?;
                erf.build_year = c.u32()? + 1900;
                erf.build_day = c.u32()?;
                c.u32()?; // Unknown, 0xFFFFFFFF.
                let flags = c.u32()?;
                erf.encryption = Encryption::from_flags(flags);
                erf.compression = Compression::from_flags(flags);
                erf.module_id = c.u32()?;
                erf.password_digest = Some(c.take(16)?.try_into().unwrap());
                for _ in 0..count {
                    let name = c.utf16_fixed(64)?;
                    let offset = c.u32()?;
                    let packed_size = c.u32()?;
                    let size = c.u32()?;
                    erf.entries.push(ErfEntry {
                        name: Some(name),
                        name_hash: None,
                        type_hash: None,
                        offset,
                        packed_size,
                        size,
                    });
                }
            }
            ErfVersion::V30 => {
                let string_table_size = c.u32()? as usize;
                let count = c.u32()?;
                let flags = c.u32()?;
                erf.encryption = Encryption::from_flags(flags);
                erf.compression = Compression::from_flags(flags);
                erf.module_id = c.u32()?;
                erf.password_digest = Some(c.take(16)?.try_into().unwrap());
                let strings = c.take(string_table_size)?;
                for _ in 0..count {
                    let name_offset = c.u32()? as i32;
                    let name = if name_offset >= 0 {
                        let start = name_offset as usize;
                        if start >= strings.len() {
                            return Err(malformed(path, "name offset outside the string table"));
                        }
                        let end = strings[start..]
                            .iter()
                            .position(|&b| b == 0)
                            .map_or(strings.len(), |p| start + p);
                        Some(String::from_utf8_lossy(&strings[start..end]).into_owned())
                    } else {
                        None
                    };
                    let name_hash = c.u64()?;
                    let type_hash = c.u32()?;
                    let offset = c.u32()?;
                    let packed_size = c.u32()?;
                    let size = c.u32()?;
                    erf.entries.push(ErfEntry {
                        name,
                        name_hash: Some(name_hash),
                        type_hash: Some(type_hash),
                        offset,
                        packed_size,
                        size,
                    });
                }
            }
        }

        for e in &erf.entries {
            if e.offset as u64 + e.packed_size as u64 > file_len {
                return Err(malformed(
                    path,
                    format!("entry {} runs past end of file", e.display_name()),
                ));
            }
        }
        Ok(erf)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_encrypted(&self) -> bool {
        self.encryption != Encryption::None
    }

    /// Finds an entry by name, case-insensitively.
    pub fn find(&self, name: &str) -> Option<&ErfEntry> {
        self.entries.iter().find(|e| {
            e.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
    }

    /// Reads the raw stored bytes of an entry, without decryption or decompression.
    pub fn read_raw(&self, entry: &ErfEntry) -> Result<Vec<u8>> {
        let mut data = vec![0u8; entry.packed_size as usize];
        let mut file = self.file.lock().unwrap_or_else(|p| p.into_inner());
        file.seek(SeekFrom::Start(entry.offset as u64))
            .and_then(|_| file.read_exact(&mut data))
            .map_err(|e| io_err(&self.path, e))?;
        Ok(data)
    }

    /// The password read from the player's own `DAOrigins.exe` when it matches the header's
    /// digest (see [`crate::gog_dlc`]; GOG Ultimate Edition layout only, provisional).
    pub fn installed_key(&self) -> Option<&DaoKey> {
        self.installed_key
            .get_or_init(|| match self.encryption {
                Encryption::Xor | Encryption::BlowfishDao => {
                    crate::gog_dlc::key(&self.path, self.module_id, self.password_digest.as_ref()?)
                }
                _ => None,
            })
            .as_ref()
    }

    /// Reads an entry, decrypting and decompressing as needed.
    /// An explicit key takes precedence over the password from the local installation.
    pub fn read(&self, entry: &ErfEntry, key: Option<&DaoKey>) -> Result<Vec<u8>> {
        if !self.is_encrypted() {
            return self.decompress(entry, self.read_raw(entry)?);
        }

        let encrypted = || Error::Encrypted {
            path: self.path.clone(),
            name: entry.display_name(),
        };
        let wrong_key = || Error::WrongKey {
            path: self.path.clone(),
        };
        let key = key.or_else(|| self.installed_key()).ok_or_else(encrypted)?;
        let digest = self.password_digest.as_ref().ok_or_else(wrong_key)?;
        let mut data = self.read_raw(entry)?;
        match self.encryption {
            Encryption::Xor | Encryption::BlowfishDao => {
                if !key.matches_digest(digest) {
                    return Err(wrong_key());
                }
                let bytes = key
                    .blowfish_key()
                    .filter(|b| *b != [0; 8])
                    .ok_or_else(wrong_key)?;
                if self.encryption == Encryption::Xor {
                    // CBWCXor uses an inclusive end index. The zero-initialized byte
                    // after the eight-byte numeric key is the ninth byte of the cycle.
                    let mut mask = [0; 9];
                    mask[..8].copy_from_slice(&bytes);
                    for (byte, mask) in data.iter_mut().zip(mask.into_iter().cycle()) {
                        *byte ^= mask;
                    }
                } else {
                    self.check_blowfish_size(entry)?;
                    blowfish_ecb_decrypt(&bytes, &mut data);
                }
            }
            Encryption::BlowfishDa2 => {
                self.check_blowfish_size(entry)?;
                let cipher = key.da2_cipher(digest).ok_or_else(wrong_key)?;
                decrypt_blocks(&cipher, &mut data);
            }
            _ => return Err(encrypted()),
        }
        if self.compression == Compression::None {
            if data.len() < entry.size as usize {
                return Err(malformed(
                    &self.path,
                    "decrypted entry is shorter than its declared size",
                ));
            }
            data.truncate(entry.size as usize);
        }
        self.decompress(entry, data)
    }

    fn check_blowfish_size(&self, entry: &ErfEntry) -> Result<()> {
        if entry.packed_size & 7 != 0 {
            return Err(malformed(
                &self.path,
                "Blowfish entry length is not a multiple of eight",
            ));
        }
        Ok(())
    }

    fn decompress(&self, entry: &ErfEntry, data: Vec<u8>) -> Result<Vec<u8>> {
        use flate2::read::{DeflateDecoder, ZlibDecoder};

        let fail = |reason: String| Error::Decompress {
            path: self.path.clone(),
            name: entry.display_name(),
            reason,
        };
        // Stored entries in compressed archives have packed == unpacked size.
        if self.compression == Compression::None || entry.packed_size == entry.size {
            return Ok(data);
        }
        let mut out = Vec::with_capacity(entry.size as usize);
        let result = match self.compression {
            Compression::BiowareZlib => match data.split_first() {
                Some((_window, rest)) => DeflateDecoder::new(rest).read_to_end(&mut out),
                None => return Err(fail("empty entry".into())),
            },
            Compression::HeaderlessZlib => DeflateDecoder::new(&data[..]).read_to_end(&mut out),
            Compression::StandardZlib => ZlibDecoder::new(&data[..]).read_to_end(&mut out),
            other => {
                return Err(Error::UnsupportedCompression {
                    path: self.path.clone(),
                    name: entry.display_name(),
                    compression: other.code(),
                });
            }
        };
        result.map_err(|e| fail(e.to_string()))?;
        if out.len() != entry.size as usize {
            return Err(fail(format!(
                "expected {} bytes, got {}",
                entry.size,
                out.len()
            )));
        }
        Ok(out)
    }
}

fn read_up_to(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match file.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn utf16(s: &str, bytes: usize) -> Vec<u8> {
        let mut v: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        v.resize(bytes, 0);
        v
    }

    fn temp_file(name: &str, data: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("thedas-vfs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::File::create(&path)
            .unwrap()
            .write_all(data)
            .unwrap();
        path
    }

    #[test]
    fn sniffs_versions() {
        assert_eq!(sniff_version(&utf16("ERF V2.0", 16)), Some(ErfVersion::V20));
        assert_eq!(sniff_version(&utf16("ERF V3.0", 16)), Some(ErfVersion::V30));
        assert_eq!(
            sniff_version(b"ERF V2.1\0\0\0\0\0\0\0\0"),
            Some(ErfVersion::V21)
        );
        assert_eq!(sniff_version(b"GFF V4.0PC  G2DA"), None);
    }

    #[test]
    fn reads_v20() {
        let payload_a = b"hello";
        let payload_b = b"world!!";
        let mut erf = utf16("ERF V2.0", 16);
        erf.extend(2u32.to_le_bytes());
        erf.extend(109u32.to_le_bytes());
        erf.extend(42u32.to_le_bytes());
        erf.extend(0xFFFF_FFFFu32.to_le_bytes());
        let data_start = 32 + 2 * 72;
        erf.extend(utf16("a.txt", 64));
        erf.extend((data_start as u32).to_le_bytes());
        erf.extend((payload_a.len() as u32).to_le_bytes());
        erf.extend(utf16("B.gda", 64));
        erf.extend(((data_start + payload_a.len()) as u32).to_le_bytes());
        erf.extend((payload_b.len() as u32).to_le_bytes());
        erf.extend(payload_a);
        erf.extend(payload_b);

        let path = temp_file("v20.erf", &erf);
        let archive = Erf::open(&path).unwrap();
        assert_eq!(archive.version, ErfVersion::V20);
        assert_eq!(archive.build_year, 2009);
        assert_eq!(archive.entries.len(), 2);
        let b = archive.find("b.GDA").unwrap();
        assert_eq!(archive.read(b, None).unwrap(), payload_b);
    }

    #[test]
    fn reads_v21_zlib() {
        use flate2::{Compression as Level, write::ZlibEncoder};
        let plain = b"some plot file contents, repeated, repeated, repeated";
        let mut enc = ZlibEncoder::new(Vec::new(), Level::default());
        enc.write_all(plain).unwrap();
        let packed = enc.finish().unwrap();

        let mut erf = b"ERF V2.1\0\0\0\0\0\0\0\0".to_vec();
        erf.extend(1u32.to_le_bytes());
        erf.extend(0u32.to_le_bytes());
        erf.extend(110u16.to_le_bytes());
        erf.extend(79u16.to_le_bytes());
        erf.extend(0x0000_FFFFu32.to_le_bytes());
        let mut name = b"plt_test.plo".to_vec();
        name.resize(32, 0);
        erf.extend(name);
        erf.extend(76u32.to_le_bytes());
        erf.extend((packed.len() as u32).to_le_bytes());
        erf.extend((plain.len() as u32).to_le_bytes());
        erf.extend(&packed);

        let path = temp_file("v21.rim", &erf);
        let archive = Erf::open(&path).unwrap();
        assert_eq!(archive.version, ErfVersion::V21);
        assert_eq!(archive.build_year, 2010);
        let e = &archive.entries[0];
        assert_eq!(archive.read(e, None).unwrap(), plain);
    }

    fn write_encrypted_v22(path: &Path, digest: [u8; 16], payload: &[u8], size: u32, kind: u32) {
        let mut bytes = utf16("ERF V2.2", 16);
        for word in [1u32, 110, 1, u32::MAX, kind << 4, 100] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend(digest);
        bytes.extend(utf16("fixture.txt", 64));
        bytes.extend(132u32.to_le_bytes());
        bytes.extend((payload.len() as u32).to_le_bytes());
        bytes.extend(size.to_le_bytes());
        bytes.extend(payload);
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn reads_xor_using_the_original_inclusive_key_index() {
        use md5::{Digest, Md5};

        // Explicit ciphertext for a nine-byte cycle, including the zero at index eight.
        let cipher = [
            0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x69, 0x6b, 0x69,
        ];
        let path = temp_file("xor.erf", b"");
        write_encrypted_v22(
            &path,
            Md5::digest(b"578437695752307201").into(),
            &cipher,
            11,
            1,
        );
        let archive = Erf::open(&path).unwrap();
        let entry = &archive.entries[0];
        assert_eq!(
            archive
                .read(entry, Some(&DaoKey::new("578437695752307201")))
                .unwrap(),
            b"abcdefghijk"
        );
        assert!(matches!(
            archive.read(entry, Some(&DaoKey::new("1"))),
            Err(Error::WrongKey { .. })
        ));
    }

    #[test]
    fn reads_da2_blowfish_with_a_raw_password_and_challenge_digest() {
        // Ciphertext and digest produced independently with OpenSSL Blowfish ECB.
        let password = [0, 255, 128, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
        let digest = [
            0x87, 0x34, 0x4d, 0x5d, 0x6d, 0x8d, 0xa6, 0xb5, 0xd6, 0xac, 0xd7, 0xf4, 0x2e, 0x32,
            0x1e, 0x7f,
        ];
        let cipher = [
            0x7b, 0x6c, 0x94, 0x8a, 0xb0, 0x9e, 0xa4, 0x31, 0x06, 0xd1, 0x79, 0x07, 0x7e, 0x74,
            0x82, 0x2c,
        ];
        // V3.0 has no string table in this fixture and one hash-only entry.
        let mut bytes = utf16("ERF V3.0", 16);
        for word in [0u32, 1, 0x30, 1] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend(digest);
        bytes.extend(u32::MAX.to_le_bytes());
        bytes.extend(1u64.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(76u32.to_le_bytes());
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(11u32.to_le_bytes());
        bytes.extend(cipher);

        let path = temp_file("type3.erf", &bytes);
        let archive = Erf::open(&path).unwrap();
        let entry = &archive.entries[0];
        assert_eq!(
            archive
                .read(entry, Some(&DaoKey::from_bytes(password)))
                .unwrap(),
            b"sample data"
        );
        assert!(matches!(
            archive.read(entry, Some(&DaoKey::new("invalid"))),
            Err(Error::WrongKey { .. })
        ));
        assert!(matches!(
            archive.read(entry, None),
            Err(Error::Encrypted { .. })
        ));
    }

    #[test]
    fn rejects_truncated_table() {
        let mut erf = utf16("ERF V2.0", 16);
        erf.extend(1000u32.to_le_bytes());
        erf.extend([0u8; 12]);
        let path = temp_file("truncated.erf", &erf);
        assert!(matches!(Erf::open(&path), Err(Error::Malformed { .. })));
    }
}
