//! Service-backend handlers for opening and applying a setup file.

use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use jiff::civil::Date;
use zeroize::Zeroizing;

use crate::secrets::{self, SecretStore};
use crate::setup_file::{SetupError, SetupSummary, open_file};

/// The relay key of the opened setup file, by token, waiting to be applied.
/// An empty key means the file was opened and has no relay key.
pub type SetupSlot = Arc<Mutex<Option<(u64, Zeroizing<String>)>>>;

/// The largest setup file Chaat reads.
const MAX_FILE: u64 = 64 * 1024;

fn lock(slot: &SetupSlot) -> MutexGuard<'_, Option<(u64, Zeroizing<String>)>> {
    slot.lock().unwrap_or_else(|p| p.into_inner())
}

/// Held in place of a key when `token` was discarded while its open was still
/// running; a finishing open for that token or an older one stores nothing.
const DISCARDED: &str = "\u{0}discarded";

fn clear_older(slot: &SetupSlot, token: u64) {
    let mut held = lock(slot);
    if held
        .as_ref()
        .is_some_and(|(held_token, _)| *held_token < token)
    {
        *held = None;
    }
}

/// Reads at most `MAX_FILE` bytes; `None` when the file is larger or
/// unreadable.
fn read_limited(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_FILE).then_some(bytes)
}

/// Opens the file at `path` and keeps its relay key in `slot` under `token`.
/// Errors carry fixed texts only, never the path or file content.
pub fn open(
    path: &Path,
    passphrase: &str,
    token: u64,
    today: Date,
    slot: &SetupSlot,
) -> Result<SetupSummary, String> {
    // A new open replaces what an older one held, never a newer token's.
    clear_older(slot, token);
    let bytes = read_limited(path).ok_or_else(|| SetupError::NotASetupFile.to_string())?;
    let (summary, key) = open_file(&bytes, passphrase, today).map_err(|error| error.to_string())?;
    let mut held = lock(slot);
    if held
        .as_ref()
        .is_none_or(|(held_token, _)| *held_token < token)
    {
        *held = Some((token, key.unwrap_or_default()));
    }
    Ok(summary)
}

/// Stores the held relay key for `token`; false if nothing is held for it or
/// the store refuses it. A file without a key applies and touches nothing.
pub fn apply(token: u64, slot: &SetupSlot, secrets: &dyn SecretStore) -> bool {
    let mut held = lock(slot);
    if held
        .as_ref()
        .is_none_or(|(held_token, _)| *held_token != token)
    {
        return false;
    }
    let Some((_, key)) = held.take() else {
        return false;
    };
    if key.as_str() == DISCARDED {
        return false;
    }
    if key.is_empty() {
        return true;
    }
    secrets.write(secrets::RELAY_KEY, &key).is_ok()
}

/// Drops what is held for `token`.
pub fn discard(token: u64, slot: &SetupSlot) {
    let mut held = lock(slot);
    match held.as_ref() {
        Some((held_token, _)) if *held_token == token => *held = None,
        // Nothing held: its open may still be running. Leave a marker.
        None => *held = Some((token, Zeroizing::new(DISCARDED.to_owned()))),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;
    use crate::setup_file::{KdfParams, SetupPayload, seal_file};

    const KDF: KdfParams = KdfParams {
        m_kib: 8192,
        t: 1,
        p: 1,
    };
    const KEY: &str = "0123456789abcdef0123";

    fn today() -> Date {
        Date::new(2026, 11, 21).unwrap()
    }

    fn payload(key: Option<&str>) -> SetupPayload {
        SetupPayload {
            format: 1,
            created: "2026-11-21T10:00:00Z".into(),
            org_label: "Contoso".into(),
            client_id: "11111111-1111-1111-1111-111111111111".into(),
            tenant: "organizations".into(),
            relay_url: key.map(|_| "https://relay.example.com".into()),
            relay_key: key.map(|k| Zeroizing::new(k.to_owned())),
            import_by: None,
        }
    }

    /// Writes a sealed file; returns the directory guard, path and passphrase.
    fn file(key: Option<&str>) -> (tempfile::TempDir, std::path::PathBuf, String) {
        let random: Vec<u8> = (0..120u8)
            .map(|i| i.wrapping_mul(7).wrapping_add(3))
            .collect();
        let (bytes, pass) = seal_file(&payload(key), &random, KDF).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.chaatsetup");
        std::fs::write(&path, bytes).unwrap();
        (dir, path, pass)
    }

    fn slot() -> SetupSlot {
        SetupSlot::default()
    }

    #[test]
    fn open_then_apply_stores_the_key() {
        let (_dir, path, pass) = file(Some(KEY));
        let slot = slot();
        let summary = open(&path, &pass, 1, today(), &slot).unwrap();
        assert_eq!(summary.org_label, "Contoso");
        assert!(summary.relay_key_included);
        let store = MemoryStore::default();
        assert!(apply(1, &slot, &store));
        assert_eq!(
            store.read(secrets::RELAY_KEY).unwrap().as_deref(),
            Some(KEY)
        );
        assert!(lock(&slot).is_none());
    }

    #[test]
    fn second_apply_false() {
        let (_dir, path, pass) = file(Some(KEY));
        let slot = slot();
        open(&path, &pass, 1, today(), &slot).unwrap();
        let store = MemoryStore::default();
        assert!(apply(1, &slot, &store));
        assert!(!apply(1, &slot, &store));
        // The same for a file without a key.
        let (_dir, path, pass) = file(None);
        open(&path, &pass, 2, today(), &slot).unwrap();
        assert!(apply(2, &slot, &store));
        assert!(!apply(2, &slot, &store));
        assert!(!apply(9, &slot, &store), "empty slot");
    }

    #[test]
    fn apply_without_key_true_and_store_untouched() {
        let (_dir, path, pass) = file(None);
        let slot = slot();
        let summary = open(&path, &pass, 5, today(), &slot).unwrap();
        assert!(!summary.relay_key_included);
        let store = MemoryStore::default();
        store
            .write(secrets::RELAY_KEY, "existing-key-0123456789")
            .unwrap();
        assert!(apply(5, &slot, &store));
        assert_eq!(
            store.read(secrets::RELAY_KEY).unwrap().as_deref(),
            Some("existing-key-0123456789")
        );
    }

    #[test]
    fn wrong_token_applies_nothing() {
        let (_dir, path, pass) = file(Some(KEY));
        let slot = slot();
        open(&path, &pass, 1, today(), &slot).unwrap();
        let store = MemoryStore::default();
        assert!(!apply(2, &slot, &store));
        assert_eq!(store.read(secrets::RELAY_KEY).unwrap(), None);
        discard(2, &slot);
        assert!(lock(&slot).is_some(), "another token's discard is ignored");
        discard(1, &slot);
        assert!(lock(&slot).is_none());
    }

    #[test]
    fn new_open_replaces_slot() {
        let (_dir, path, pass) = file(Some(KEY));
        let slot = slot();
        open(&path, &pass, 1, today(), &slot).unwrap();
        open(&path, &pass, 2, today(), &slot).unwrap();
        let store = MemoryStore::default();
        assert!(!apply(1, &slot, &store));
        // A failed open leaves nothing held.
        assert!(open(&path, "wrong-words", 3, today(), &slot).is_err());
        assert!(lock(&slot).is_none());
        assert!(!apply(2, &slot, &store));
    }

    #[test]
    fn older_open_finishing_last_does_not_overwrite() {
        let (_dir, path, pass) = file(Some(KEY));
        let slot = slot();
        open(&path, &pass, 2, today(), &slot).unwrap();
        open(&path, &pass, 1, today(), &slot).unwrap();
        let store = MemoryStore::default();
        assert!(!apply(1, &slot, &store));
        assert!(apply(2, &slot, &store));
    }

    #[test]
    fn discarded_inflight_open_leaves_slot_empty() {
        let (_dir, path, pass) = file(Some(KEY));
        let slot = slot();
        discard(1, &slot);
        open(&path, &pass, 1, today(), &slot).unwrap();
        let store = MemoryStore::default();
        assert!(!apply(1, &slot, &store));
        assert_eq!(store.read(secrets::RELAY_KEY).unwrap(), None);
        // A newer open still works.
        open(&path, &pass, 2, today(), &slot).unwrap();
        assert!(apply(2, &slot, &store));
    }

    #[test]
    fn oversize_file_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.chaatsetup");
        std::fs::write(&path, vec![0u8; 64 * 1024 + 1]).unwrap();
        let slot = slot();
        assert_eq!(
            open(&path, "x", 1, today(), &slot),
            Err("This isn't a Chaat setup file.".to_owned())
        );
        let missing = dir.path().join("missing.chaatsetup");
        assert_eq!(
            open(&missing, "x", 1, today(), &slot),
            Err("This isn't a Chaat setup file.".to_owned())
        );
    }

    #[test]
    fn wrong_passphrase_text_has_no_path() {
        let (_dir, path, _) = file(Some(KEY));
        let message = open(&path, "wrong-words", 1, today(), &slot()).unwrap_err();
        assert_eq!(
            message,
            "This file can't be opened with that passphrase, or it was changed."
        );
    }
}
