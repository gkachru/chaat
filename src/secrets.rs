//! Refresh tokens in the operating system's credential store: the Keychain
//! on macOS, the Credential Manager on Windows, the Secret Service (GNOME
//! Keyring, KWallet) on Linux.
//!
//! Only refresh tokens are stored. Access tokens live in memory for their
//! hour. Errors from the native stores are reduced to fixed categories,
//! because provider messages can quote platform data.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const SERVICE: &str = "app.chaat.Chaat";

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("the credential store is locked")]
    Locked,
    #[error("the credential store is unavailable")]
    Unavailable,
}

/// A place for secrets. Calls may block (D-Bus, Keychain prompts): run them
/// off the interface thread.
pub trait SecretStore: Send + Sync {
    fn read(&self, key: &str) -> Result<Option<String>, SecretError>;
    fn write(&self, key: &str, secret: &str) -> Result<(), SecretError>;
    fn delete(&self, key: &str) -> Result<(), SecretError>;
}

/// The push relay's access key (docs/realtime.md). One for all accounts.
pub const RELAY_KEY: &str = "relay-key";

/// The refresh token's key for one account.
pub fn refresh_key(account: &str) -> String {
    format!("refresh-token:{account}")
}

fn native_error(error: keyring_core::Error) -> SecretError {
    match error {
        keyring_core::Error::NoStorageAccess(_) => SecretError::Locked,
        _ => SecretError::Unavailable,
    }
}

/// The platform's own store.
#[derive(Default)]
pub struct NativeStore {
    store: Mutex<Option<Arc<keyring_core::api::CredentialStore>>>,
}

impl NativeStore {
    fn entry(&self, key: &str) -> Result<keyring_core::Entry, SecretError> {
        let mut store = self.store.lock().unwrap_or_else(|p| p.into_inner());
        if store.is_none() {
            #[cfg(target_os = "linux")]
            let made = zbus_secret_service_keyring_store::Store::new();
            #[cfg(target_os = "macos")]
            let made = apple_native_keyring_store::keychain::Store::new();
            #[cfg(windows)]
            let made = windows_native_keyring_store::Store::new();
            #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
            let made: Result<
                Arc<keyring_core::api::CredentialStore>,
                keyring_core::Error,
            > = Err(keyring_core::Error::NoDefaultStore);
            *store = Some(made.map_err(native_error)?);
        }
        store
            .as_ref()
            .ok_or(SecretError::Unavailable)?
            .build(SERVICE, key, None)
            .map_err(native_error)
    }
}

impl SecretStore for NativeStore {
    fn read(&self, key: &str) -> Result<Option<String>, SecretError> {
        match self.entry(key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(error) => Err(native_error(error)),
        }
    }

    fn write(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.entry(key)?.set_password(secret).map_err(native_error)
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(error) => Err(native_error(error)),
        }
    }
}

/// The platform's store, ready for secrets of any length.
pub fn native() -> Arc<dyn SecretStore> {
    #[cfg(windows)]
    {
        Arc::new(Split::new(NativeStore::default(), WINDOWS_PART))
    }
    #[cfg(not(windows))]
    {
        Arc::new(NativeStore::default())
    }
}

/// Characters per Credential Manager entry. An entry holds at most 2560
/// bytes and the store writes UTF-16, so 1280 characters; Entra refresh
/// tokens are often longer.
#[cfg(windows)]
const WINDOWS_PART: usize = 1200;

/// Marks an entry that only says where the parts of a long secret are:
/// `wren-parts:<generation>:<count>`.
const PARTS_MARK: &str = "wren-parts:";

/// Stores secrets longer than `part` characters across numbered entries.
///
/// A rewrite puts the parts under the other generation before pointing the
/// main entry at them, so an interrupted write leaves the old secret whole.
pub struct Split<S> {
    inner: S,
    part: usize,
}

impl<S: SecretStore> Split<S> {
    pub fn new(inner: S, part: usize) -> Self {
        Self {
            inner,
            part: part.max(1),
        }
    }

    fn part_key(key: &str, generation: u8, index: usize) -> String {
        format!("{key}#{generation}.{index}")
    }

    fn header(value: &str) -> Option<(u8, usize)> {
        let (generation, count) = value.strip_prefix(PARTS_MARK)?.split_once(':')?;
        Some((generation.parse().ok()?, count.parse().ok()?))
    }

    fn delete_parts(&self, key: &str, generation: u8, count: usize) {
        for index in 0..count {
            let _ = self.inner.delete(&Self::part_key(key, generation, index));
        }
    }
}

impl<S: SecretStore> SecretStore for Split<S> {
    fn read(&self, key: &str) -> Result<Option<String>, SecretError> {
        let Some(value) = self.inner.read(key)? else {
            return Ok(None);
        };
        let Some((generation, count)) = Self::header(&value) else {
            return Ok(Some(value));
        };
        let mut secret = String::new();
        for index in 0..count {
            match self.inner.read(&Self::part_key(key, generation, index))? {
                Some(part) => secret.push_str(&part),
                // A missing part makes the secret unusable.
                None => return Ok(None),
            }
        }
        Ok(Some(secret))
    }

    fn write(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        let old = self
            .inner
            .read(key)
            .ok()
            .flatten()
            .and_then(|v| Self::header(&v));
        let chars: Vec<char> = secret.chars().collect();
        if chars.len() <= self.part && !secret.starts_with(PARTS_MARK) {
            self.inner.write(key, secret)?;
        } else {
            let generation = old.map_or(0, |(g, _)| 1 - g.min(1));
            let parts: Vec<String> = chars
                .chunks(self.part)
                .map(|c| c.iter().collect())
                .collect();
            for (index, part) in parts.iter().enumerate() {
                self.inner
                    .write(&Self::part_key(key, generation, index), part)?;
            }
            self.inner
                .write(key, &format!("{PARTS_MARK}{generation}:{}", parts.len()))?;
            if let Some((g, count)) = old
                && g != generation
            {
                self.delete_parts(key, g, count);
            }
            return Ok(());
        }
        if let Some((g, count)) = old {
            self.delete_parts(key, g, count);
        }
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        if let Some((generation, count)) = self
            .inner
            .read(key)
            .ok()
            .flatten()
            .and_then(|v| Self::header(&v))
        {
            self.delete_parts(key, generation, count);
        }
        self.inner.delete(key)
    }
}

impl<S: SecretStore> SecretStore for Arc<S> {
    fn read(&self, key: &str) -> Result<Option<String>, SecretError> {
        (**self).read(key)
    }
    fn write(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        (**self).write(key, secret)
    }
    fn delete(&self, key: &str) -> Result<(), SecretError> {
        (**self).delete(key)
    }
}

/// Secrets for this process only: demos and tests.
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemoryStore {
    fn read(&self, key: &str) -> Result<Option<String>, SecretError> {
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .cloned())
    }

    fn write(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trips() {
        let store = MemoryStore::default();
        let key = refresh_key("a.b");
        assert_eq!(store.read(&key).unwrap(), None);
        store.write(&key, "secret").unwrap();
        assert_eq!(store.read(&key).unwrap().as_deref(), Some("secret"));
        store.delete(&key).unwrap();
        assert_eq!(store.read(&key).unwrap(), None);
    }

    /// A store that refuses entries over `limit` characters, like
    /// Credential Manager.
    struct Limited {
        inner: MemoryStore,
        limit: usize,
    }

    impl SecretStore for Limited {
        fn read(&self, key: &str) -> Result<Option<String>, SecretError> {
            self.inner.read(key)
        }
        fn write(&self, key: &str, secret: &str) -> Result<(), SecretError> {
            if secret.chars().count() > self.limit {
                return Err(SecretError::Unavailable);
            }
            self.inner.write(key, secret)
        }
        fn delete(&self, key: &str) -> Result<(), SecretError> {
            self.inner.delete(key)
        }
    }

    fn entries(store: &Arc<Limited>) -> usize {
        store.inner.entries.lock().unwrap().len()
    }

    #[test]
    fn long_secrets_are_split_across_entries() {
        let limited = Arc::new(Limited {
            inner: MemoryStore::default(),
            limit: 16,
        });
        let store = Split::new(Arc::clone(&limited), 16);
        let key = refresh_key("a.b");
        let long: String = "0123456789".repeat(3) + "abc";

        store.write(&key, &long).unwrap();
        assert_eq!(store.read(&key).unwrap().as_deref(), Some(long.as_str()));
        assert_eq!(entries(&limited), 4);

        // A rotation to a different length replaces the parts.
        let rotated: String = "z".repeat(25);
        store.write(&key, &rotated).unwrap();
        assert_eq!(store.read(&key).unwrap().as_deref(), Some(rotated.as_str()));
        assert_eq!(entries(&limited), 3);

        // Back to a short secret: no parts left behind.
        store.write(&key, "short").unwrap();
        assert_eq!(store.read(&key).unwrap().as_deref(), Some("short"));
        assert_eq!(entries(&limited), 1);

        store.write(&key, &long).unwrap();
        store.delete(&key).unwrap();
        assert_eq!(store.read(&key).unwrap(), None);
        assert_eq!(entries(&limited), 0);
    }

    #[test]
    fn a_missing_part_reads_as_no_secret() {
        let memory = Arc::new(MemoryStore::default());
        let store = Split::new(Arc::clone(&memory), 4);
        let key = refresh_key("a.b");
        store.write(&key, "abcdefghij").unwrap();
        memory.delete(&format!("{key}#0.1")).unwrap();
        assert_eq!(store.read(&key).unwrap(), None);
    }

    #[test]
    fn an_interrupted_rewrite_keeps_the_old_secret() {
        let memory = Arc::new(MemoryStore::default());
        let store = Split::new(Arc::clone(&memory), 4);
        let key = refresh_key("a.b");
        store.write(&key, "abcdefghij").unwrap();
        // The next write's parts land under the other generation; until the
        // main entry moves, the old secret still reads whole.
        memory.write(&format!("{key}#1.0"), "XXXX").unwrap();
        assert_eq!(store.read(&key).unwrap().as_deref(), Some("abcdefghij"));
    }
}
