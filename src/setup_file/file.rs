//! Seal and open whole setup files.

use jiff::civil::Date;
use zeroize::Zeroizing;

use super::format::{SetupError, open_bytes, seal_bytes};
use super::kdf::KdfParams;
use super::passphrase;
use super::payload::{SetupPayload, SetupSummary, validate};

/// Seals `p` with a generated passphrase; returns the file bytes and the
/// passphrase. `random`: bytes 0..16 are the salt, 16..40 the nonce, the rest
/// the passphrase draws.
pub fn seal_file(
    p: &SetupPayload,
    random: &[u8],
    kdf: KdfParams,
) -> Result<(Vec<u8>, String), SetupError> {
    let salt: [u8; 16] = random
        .get(..16)
        .and_then(|b| b.try_into().ok())
        .ok_or(SetupError::Invalid)?;
    let nonce: [u8; 24] = random
        .get(16..40)
        .and_then(|b| b.try_into().ok())
        .ok_or(SetupError::Invalid)?;
    let words = random.get(40..).ok_or(SetupError::Invalid)?;
    let pass = passphrase::generate(words).ok_or(SetupError::Invalid)?;
    let plain = Zeroizing::new(serde_json::to_vec(p).map_err(|_| SetupError::Invalid)?);
    let bytes = seal_bytes(&plain, &pass, kdf, salt, nonce)?;
    Ok((bytes, pass))
}

/// Opens a setup file: the summary and the relay key, if one is included.
pub fn open_file(
    bytes: &[u8],
    passphrase: &str,
    today: Date,
) -> Result<(SetupSummary, Option<Zeroizing<String>>), SetupError> {
    let plain = open_bytes(bytes, passphrase)?;
    let mut payload: SetupPayload =
        serde_json::from_slice(&plain).map_err(|_| SetupError::Invalid)?;
    if payload.format > 1 {
        return Err(SetupError::NewerFormat);
    }
    let summary = validate(&payload, today)?;
    Ok((summary, payload.relay_key.take()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: KdfParams = KdfParams {
        m_kib: 8192,
        t: 1,
        p: 1,
    };
    const GUID: &str = "11111111-2222-3333-4444-555555555555";

    fn today() -> Date {
        Date::constant(2026, 11, 21)
    }

    fn random() -> Vec<u8> {
        (0..128u32).map(|i| (i * 37 % 251) as u8).collect()
    }

    fn payload(key: Option<&str>) -> SetupPayload {
        SetupPayload {
            format: 1,
            created: "2026-11-21T10:00:00Z".into(),
            org_label: "Acme".into(),
            client_id: GUID.into(),
            tenant: "organizations".into(),
            relay_url: key.map(|_| "https://relay.example.com".into()),
            relay_key: key.map(|k| Zeroizing::new(k.to_string())),
            import_by: None,
        }
    }

    #[test]
    fn round_trip_with_key() {
        let key = "k".repeat(43);
        let (bytes, pass) = seal_file(&payload(Some(&key)), &random(), K).unwrap();
        let (summary, got) = open_file(&bytes, &pass, today()).unwrap();
        assert!(summary.relay_key_included);
        assert_eq!(summary.org_label, "Acme");
        assert_eq!(got.as_deref().map(String::as_str), Some(key.as_str()));
    }

    #[test]
    fn round_trip_without_key() {
        let (bytes, pass) = seal_file(&payload(None), &random(), K).unwrap();
        let (summary, got) = open_file(&bytes, &pass, today()).unwrap();
        assert!(!summary.relay_key_included);
        assert!(got.is_none());
    }

    #[test]
    fn too_little_randomness_is_invalid() {
        assert_eq!(
            seal_file(&payload(None), &[0u8; 10], K).unwrap_err(),
            SetupError::Invalid
        );
        assert_eq!(
            seal_file(&payload(None), &[0u8; 45], K).unwrap_err(),
            SetupError::Invalid
        );
    }

    #[test]
    fn expired_file_is_refused() {
        let mut p = payload(None);
        p.import_by = Some("2026-11-20".into());
        let (bytes, pass) = seal_file(&p, &random(), K).unwrap();
        assert_eq!(
            open_file(&bytes, &pass, today()).unwrap_err(),
            SetupError::Expired("2026-11-20".into())
        );
    }

    #[test]
    fn payload_format_2_is_newer() {
        let mut p = payload(None);
        p.format = 2;
        let (bytes, pass) = seal_file(&p, &random(), K).unwrap();
        assert_eq!(
            open_file(&bytes, &pass, today()).unwrap_err(),
            SetupError::NewerFormat
        );
    }

    #[test]
    fn non_json_payload_is_invalid() {
        let bytes = seal_bytes(b"not json", "a-b", K, [1; 16], [2; 24]).unwrap();
        assert_eq!(
            open_file(&bytes, "a-b", today()).unwrap_err(),
            SetupError::Invalid
        );
    }
}
