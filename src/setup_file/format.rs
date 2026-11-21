//! The file container: header, KDF parameters, AEAD.

use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use super::kdf::{KdfParams, derive_key};

/// Why a setup file could not be read or written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupError {
    NotASetupFile,
    NewerFormat,
    WrongPassphraseOrChanged,
    Invalid,
    /// The file's import-by date has passed (the date, `YYYY-MM-DD`).
    Expired(String),
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotASetupFile => f.write_str("This isn't a Chaat setup file."),
            Self::NewerFormat => f.write_str("This file needs a newer version of Chaat."),
            Self::WrongPassphraseOrChanged => {
                f.write_str("This file can't be opened with that passphrase, or it was changed.")
            }
            Self::Invalid => f.write_str("This file can't be read. Ask for a new one."),
            Self::Expired(date) => {
                write!(f, "This setup file expired on {date}. Ask for a new one.")
            }
        }
    }
}

impl std::error::Error for SetupError {}

/// The format this version writes and the newest it reads.
const FORMAT: u16 = 1;
/// The oldest format this version still reads.
const OLDEST_FORMAT: u16 = 1;
/// Larger inputs are not setup files; refused before any key work.
const MAX_FILE_LEN: usize = 64 * 1024;

const MAGIC: &[u8; 10] = b"CHAATSETUP";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
/// Offset of the Argon2 parameters (after the magic and the u16 format).
const KDF_AT: usize = MAGIC.len() + 2;
const SALT_AT: usize = KDF_AT + 12;
const NONCE_AT: usize = SALT_AT + SALT_LEN;
/// magic + format u16 + m u32 + t u32 + p u32 + salt + nonce.
const HEADER_LEN: usize = NONCE_AT + NONCE_LEN;

/// Encrypts `plain` into a setup file container. The whole header is the
/// associated data. `salt` and `nonce` must be fresh random values per file.
pub fn seal_bytes(
    plain: &[u8],
    passphrase: &str,
    kdf: KdfParams,
    salt: [u8; 16],
    nonce: [u8; 24],
) -> Result<Vec<u8>, SetupError> {
    if HEADER_LEN + plain.len() + TAG_LEN > MAX_FILE_LEN {
        return Err(SetupError::Invalid);
    }
    let mut out = Vec::with_capacity(HEADER_LEN + plain.len() + TAG_LEN);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(&kdf.m_kib.to_le_bytes());
    out.extend_from_slice(&kdf.t.to_le_bytes());
    out.extend_from_slice(&kdf.p.to_le_bytes());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    let key = derive_key(passphrase, kdf, &salt).map_err(|e| match e {
        // A bad cost on the writing side is a caller error, not a passphrase one.
        SetupError::WrongPassphraseOrChanged => SetupError::Invalid,
        other => other,
    })?;
    let cipher = XChaCha20Poly1305::new(key.as_slice().into());
    let sealed = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plain,
                aad: &out,
            },
        )
        .map_err(|_| SetupError::Invalid)?;
    out.extend_from_slice(&sealed);
    Ok(out)
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

/// Decrypts a setup file container. Size, magic, format and KDF bounds are
/// checked before any key work.
pub fn open_bytes(bytes: &[u8], passphrase: &str) -> Result<Zeroizing<Vec<u8>>, SetupError> {
    if bytes.len() > MAX_FILE_LEN || bytes.len() < HEADER_LEN || !bytes.starts_with(MAGIC) {
        return Err(SetupError::NotASetupFile);
    }
    let (header, ciphertext) = bytes.split_at(HEADER_LEN);
    let format = header
        .get(MAGIC.len()..KDF_AT)
        .and_then(|b| <[u8; 2]>::try_from(b).ok())
        .map(u16::from_le_bytes)
        .ok_or(SetupError::NotASetupFile)?;
    if format > FORMAT {
        return Err(SetupError::NewerFormat);
    }
    if format < OLDEST_FORMAT {
        return Err(SetupError::NotASetupFile);
    }
    let (Some(m_kib), Some(t), Some(p)) = (
        le_u32(header, KDF_AT),
        le_u32(header, KDF_AT + 4),
        le_u32(header, KDF_AT + 8),
    ) else {
        return Err(SetupError::NotASetupFile);
    };
    let (Some(salt), Some(nonce)) = (
        header.get(SALT_AT..NONCE_AT),
        header.get(NONCE_AT..HEADER_LEN),
    ) else {
        return Err(SetupError::NotASetupFile);
    };
    let key = derive_key(passphrase, KdfParams { m_kib, t, p }, salt)?;
    let cipher = XChaCha20Poly1305::new(key.as_slice().into());
    let plain = cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: header,
            },
        )
        .map_err(|_| SetupError::WrongPassphraseOrChanged)?;
    Ok(Zeroizing::new(plain))
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: KdfParams = KdfParams {
        m_kib: 8192,
        t: 1,
        p: 1,
    };
    const SALT: [u8; 16] = [3; 16];
    const NONCE: [u8; 24] = [5; 24];

    fn sealed() -> Vec<u8> {
        seal_bytes(b"{}", "a-b", K, SALT, NONCE).unwrap()
    }

    #[test]
    fn round_trip_with_normalized_passphrase() {
        let out = open_bytes(&sealed(), "A B").unwrap();
        assert_eq!(&out[..], b"{}");
    }

    #[test]
    fn not_a_setup_file() {
        assert_eq!(open_bytes(b"nope", "x"), Err(SetupError::NotASetupFile));
        let mut f = sealed();
        f[0] = b'X';
        assert_eq!(open_bytes(&f, "a-b"), Err(SetupError::NotASetupFile));
    }

    #[test]
    fn wrong_passphrase() {
        assert_eq!(
            open_bytes(&sealed(), "a-c"),
            Err(SetupError::WrongPassphraseOrChanged)
        );
    }

    #[test]
    fn salt_byte_flip_fails() {
        let mut f = sealed();
        f[SALT_AT] ^= 1;
        assert_eq!(
            open_bytes(&f, "a-b"),
            Err(SetupError::WrongPassphraseOrChanged)
        );
    }

    #[test]
    fn every_header_byte_is_authenticated() {
        for i in KDF_AT..HEADER_LEN {
            let mut f = sealed();
            f[i] ^= 1;
            assert!(open_bytes(&f, "a-b").is_err(), "byte {i}");
        }
    }

    #[test]
    fn ciphertext_flip_fails() {
        let mut f = sealed();
        let last = f.len() - 1;
        f[last] ^= 1;
        assert_eq!(
            open_bytes(&f, "a-b"),
            Err(SetupError::WrongPassphraseOrChanged)
        );
    }

    #[test]
    fn format_2_newer() {
        let mut f = sealed();
        f[MAGIC.len()] = 2;
        assert_eq!(open_bytes(&f, "a-b"), Err(SetupError::NewerFormat));
        f[MAGIC.len()] = 0;
        assert_eq!(open_bytes(&f, "a-b"), Err(SetupError::NotASetupFile));
    }

    #[test]
    fn over_64_kib_not_a_setup_file() {
        let mut f = sealed();
        f.resize(MAX_FILE_LEN + 1, 0);
        assert_eq!(open_bytes(&f, "a-b"), Err(SetupError::NotASetupFile));
        // Exactly at the limit is still read (and fails on the tag).
        f.truncate(MAX_FILE_LEN);
        assert_eq!(
            open_bytes(&f, "a-b"),
            Err(SetupError::WrongPassphraseOrChanged)
        );
    }

    #[test]
    fn crafted_cost_refused_without_key_work() {
        let mut f = sealed();
        f[KDF_AT..KDF_AT + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            open_bytes(&f, "a-b"),
            Err(SetupError::WrongPassphraseOrChanged)
        );
    }

    #[test]
    fn seal_refuses_bad_cost_and_oversize() {
        let bad = KdfParams { m_kib: 1, ..K };
        assert_eq!(
            seal_bytes(b"{}", "a", bad, SALT, NONCE),
            Err(SetupError::Invalid)
        );
        let big = vec![0u8; MAX_FILE_LEN];
        assert_eq!(
            seal_bytes(&big, "a", K, SALT, NONCE),
            Err(SetupError::Invalid)
        );
    }

    #[test]
    fn display_texts_match_spec() {
        assert_eq!(
            SetupError::NotASetupFile.to_string(),
            "This isn't a Chaat setup file."
        );
        assert_eq!(
            SetupError::NewerFormat.to_string(),
            "This file needs a newer version of Chaat."
        );
        assert_eq!(
            SetupError::WrongPassphraseOrChanged.to_string(),
            "This file can't be opened with that passphrase, or it was changed."
        );
        assert_eq!(
            SetupError::Invalid.to_string(),
            "This file can't be read. Ask for a new one."
        );
        assert_eq!(
            SetupError::Expired("2026-11-20".into()).to_string(),
            "This setup file expired on 2026-11-20. Ask for a new one."
        );
    }
}
