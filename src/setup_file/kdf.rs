//! Key derivation from the passphrase (Argon2id).

use std::ops::RangeInclusive;

use argon2::{Algorithm, Argon2, Block, Params, Version};
use zeroize::Zeroizing;

use super::format::SetupError;
use super::passphrase::normalize;

/// Argon2id cost parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

/// The parameters new files are sealed with: 64 MiB, 3 passes, 1 lane.
pub const DEFAULT_KDF: KdfParams = KdfParams {
    m_kib: 65_536,
    t: 3,
    p: 1,
};

impl KdfParams {
    const M_KIB_RANGE: RangeInclusive<u32> = 8_192..=1_048_576;
    const T_RANGE: RangeInclusive<u32> = 1..=10;
    const P_RANGE: RangeInclusive<u32> = 1..=8;

    fn in_bounds(&self) -> bool {
        Self::M_KIB_RANGE.contains(&self.m_kib)
            && Self::T_RANGE.contains(&self.t)
            && Self::P_RANGE.contains(&self.p)
    }
}

/// Derives the 32-byte key for `passphrase` and `salt`. Costs outside the
/// bounds are refused before anything is allocated.
pub fn derive_key(
    passphrase: &str,
    kdf: KdfParams,
    salt: &[u8],
) -> Result<Zeroizing<[u8; 32]>, SetupError> {
    if !kdf.in_bounds() {
        return Err(SetupError::WrongPassphraseOrChanged);
    }
    let params = Params::new(kdf.m_kib, kdf.t, kdf.p, Some(32))
        .map_err(|_| SetupError::WrongPassphraseOrChanged)?;
    let count = params.block_count();
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let secret = Zeroizing::new(normalize(passphrase).into_bytes());
    // Allocate the work memory here so a failed allocation is an error, not an abort.
    let mut memory: Vec<Block> = Vec::new();
    memory
        .try_reserve_exact(count)
        .map_err(|_| SetupError::Invalid)?;
    memory.resize(count, Block::default());
    // Wiped on drop, on success and on error.
    let mut memory = Zeroizing::new(memory);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into_with_memory(&secret, salt, key.as_mut_slice(), &mut *memory)
        .map_err(|_| SetupError::WrongPassphraseOrChanged)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams {
        m_kib: 8192,
        t: 1,
        p: 1,
    };
    const SALT: [u8; 16] = [7; 16];

    #[test]
    fn default_is_in_bounds() {
        assert!(DEFAULT_KDF.in_bounds());
        assert_eq!(DEFAULT_KDF.m_kib, 65_536);
    }

    #[test]
    fn out_of_bounds_refused_before_alloc() {
        for k in [
            KdfParams {
                m_kib: 4_000_000,
                t: 1,
                p: 1,
            },
            KdfParams {
                m_kib: u32::MAX,
                t: 1,
                p: 1,
            },
            KdfParams {
                m_kib: 8191,
                t: 1,
                p: 1,
            },
            KdfParams {
                m_kib: 8192,
                t: 0,
                p: 1,
            },
            KdfParams {
                m_kib: 8192,
                t: 11,
                p: 1,
            },
            KdfParams {
                m_kib: 8192,
                t: 1,
                p: 0,
            },
            KdfParams {
                m_kib: 8192,
                t: 1,
                p: 9,
            },
            KdfParams {
                m_kib: 8192,
                t: u32::MAX,
                p: u32::MAX,
            },
        ] {
            assert_eq!(
                derive_key("a", k, &SALT),
                Err(SetupError::WrongPassphraseOrChanged)
            );
        }
    }

    #[test]
    fn normalized_input_same_key() {
        let a = derive_key("Apple banana", FAST, &SALT).unwrap();
        let b = derive_key("apple-banana", FAST, &SALT).unwrap();
        assert_eq!(*a, *b);
    }

    #[test]
    fn different_input_or_salt_changes_key() {
        let a = derive_key("apple", FAST, &SALT).unwrap();
        let b = derive_key("pear", FAST, &SALT).unwrap();
        let c = derive_key("apple", FAST, &[8; 16]).unwrap();
        assert_ne!(*a, *b);
        assert_ne!(*a, *c);
    }

    #[test]
    fn short_salt_is_an_error_not_a_panic() {
        assert!(derive_key("apple", FAST, &[1; 2]).is_err());
    }
}
