//! Service-backend handler that writes a setup file.

use std::path::Path;

use zeroize::Zeroizing;

use crate::secrets::{RELAY_KEY, SecretStore};
use crate::setup_file::{DEFAULT_KDF, KdfParams, SetupPayload, seal_file};

/// Writes the setup file; returns the generated passphrase.
///
/// The relay key comes from the credential store only: whatever the payload
/// carries is replaced. Errors are fixed texts that never quote the path.
pub fn export(
    path: &Path,
    payload: SetupPayload,
    include_relay_key: bool,
    secrets: &dyn SecretStore,
) -> Result<String, String> {
    export_with(path, payload, include_relay_key, secrets, DEFAULT_KDF)
}

fn export_with(
    path: &Path,
    mut payload: SetupPayload,
    include_relay_key: bool,
    secrets: &dyn SecretStore,
    kdf: KdfParams,
) -> Result<String, String> {
    payload.relay_key = None;
    if include_relay_key {
        if payload.relay_url.is_none() {
            return Err("The access key can only be included with a push relay address.".into());
        }
        match secrets.read(RELAY_KEY) {
            Ok(Some(key)) if !key.is_empty() => payload.relay_key = Some(Zeroizing::new(key)),
            Ok(_) => return Err("The access key is not stored on this computer.".into()),
            Err(_) => {
                return Err("The access key could not be read from the credential store.".into());
            }
        }
    }
    let mut random = Zeroizing::new([0u8; 128]);
    getrandom::fill(&mut random[..]).map_err(|_| "Could not create the setup file.".to_owned())?;
    let (bytes, passphrase) = seal_file(&payload, &random[..], kdf)
        .map_err(|_| "Could not create the setup file.".to_owned())?;
    crate::paths::write_atomically(path, &bytes).map_err(|_| {
        log::warn!("could not write the setup file");
        "The file could not be saved.".to_owned()
    })?;
    Ok(passphrase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;
    use crate::setup_file::open_file;

    const K: KdfParams = KdfParams {
        m_kib: 8192,
        t: 1,
        p: 1,
    };
    const GUID: &str = "11111111-2222-3333-4444-555555555555";

    fn payload(relay: bool) -> SetupPayload {
        SetupPayload {
            format: 1,
            created: "2026-11-21T10:00:00Z".into(),
            org_label: "Contoso".into(),
            client_id: GUID.into(),
            tenant: GUID.into(),
            expected_tenant: None,
            relay_url: relay.then(|| "https://relay.example.com".into()),
            relay_key: Some(Zeroizing::new("payload-key-must-be-ignored".into())),
            import_by: None,
        }
    }

    fn today() -> jiff::civil::Date {
        jiff::civil::date(2026, 11, 21)
    }

    fn key_store(key: &str) -> MemoryStore {
        let store = MemoryStore::default();
        store.write(RELAY_KEY, key).unwrap();
        store
    }

    #[test]
    fn key_read_from_store_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.chaatsetup");
        let store = key_store("0123456789abcdef0123456789abcdef");
        let pass = export_with(&path, payload(true), true, &store, K).unwrap();
        let (summary, key) = open_file(&std::fs::read(&path).unwrap(), &pass, today()).unwrap();
        assert!(summary.relay_key_included);
        assert_eq!(key.unwrap().as_str(), "0123456789abcdef0123456789abcdef");
    }

    #[test]
    fn a_payload_key_is_dropped_when_not_included() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.chaatsetup");
        let pass = export_with(&path, payload(true), false, &key_store("x"), K).unwrap();
        let (summary, key) = open_file(&std::fs::read(&path).unwrap(), &pass, today()).unwrap();
        assert!(!summary.relay_key_included);
        assert!(key.is_none());
    }

    #[test]
    fn a_missing_key_is_a_fixed_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.chaatsetup");
        let err = export_with(&path, payload(true), true, &MemoryStore::default(), K).unwrap_err();
        assert_eq!(err, "The access key is not stored on this computer.");
        assert!(!path.exists());
    }

    #[test]
    fn writes_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.chaatsetup");
        std::fs::write(&path, b"old").unwrap();
        export_with(&path, payload(false), false, &MemoryStore::default(), K).unwrap();
        assert!(std::fs::read(&path).unwrap().starts_with(b"CHAATSETUP"));
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 1, "no temporary file is left behind");
    }

    #[test]
    fn a_write_failure_does_not_name_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("secret-name");
        std::fs::write(&blocker, b"").unwrap();
        let err = export_with(
            &blocker.join("x.chaatsetup"),
            payload(false),
            false,
            &MemoryStore::default(),
            K,
        )
        .unwrap_err();
        assert!(!err.contains("secret-name"));
    }
}
