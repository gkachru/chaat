//! Generated word passphrases from the EFF large list.

use std::sync::OnceLock;

use zeroize::Zeroizing;

/// Words in the EFF large list (6^5).
pub const WORDS: usize = 7_776;
/// Words in a generated passphrase.
pub const PASSPHRASE_WORDS: usize = 6;

/// Draws at or above this value are skipped: 8 x 7,776, so the rest map evenly.
const LIMIT: u32 = 8 * WORDS as u32;

static LIST: &str = include_str!("../../assets/eff_large_wordlist.txt");

/// The list, parsed once. Each line is `digits<TAB>word`.
fn words() -> &'static [&'static str] {
    static PARSED: OnceLock<Vec<&'static str>> = OnceLock::new();
    PARSED.get_or_init(|| {
        LIST.lines()
            .map(|line| line.split_once('\t').map_or(line, |(_, word)| word))
            .collect()
    })
}

/// Six words joined with `-`. `random` is read as little-endian `u16` pairs;
/// values of 62,208 and above are skipped so every word is equally likely.
/// `None` when the bytes run out. Supply a few dozen bytes: a draw is skipped
/// about 5% of the time.
pub fn generate(random: &[u8]) -> Option<String> {
    let words = words();
    if words.len() != WORDS {
        return None;
    }
    let mut picked: Vec<&str> = Vec::with_capacity(PASSPHRASE_WORDS);
    for pair in random.as_chunks::<2>().0 {
        let value = u32::from(u16::from_le_bytes([pair[0], pair[1]]));
        if value >= LIMIT {
            continue;
        }
        picked.push(words[value as usize % WORDS]);
        if picked.len() == PASSPHRASE_WORDS {
            return Some(picked.join("-"));
        }
    }
    None
}

/// Trim, lowercase, split on whitespace and `-`, join with `-`.
pub fn normalize(text: &str) -> String {
    let lower = Zeroizing::new(text.to_lowercase());
    lower
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn list_has_7776_unique_words() {
        let words = words();
        assert_eq!(words.len(), WORDS);
        let unique: std::collections::BTreeSet<_> = words.iter().collect();
        assert_eq!(unique.len(), WORDS);
        assert!(words.iter().all(|w| !w.is_empty() && !w.contains('\t')));
        assert_eq!(words[0], "abacus");
        assert_eq!(words[WORDS - 1], "zoom");
    }

    #[test]
    fn six_words() {
        let random: Vec<u8> = (0..64u8)
            .map(|i| i.wrapping_mul(37).wrapping_add(11))
            .collect();
        let p = generate(&random).unwrap();
        let parts: Vec<&str> = p.split('-').collect();
        assert_eq!(parts.len(), PASSPHRASE_WORDS);
        assert!(parts.iter().all(|w| words().contains(w)));
    }

    #[test]
    fn draws_from_62208_skipped() {
        // 62,208 is skipped; 62,207 maps to index 7,775 (the last word).
        let p = generate(&bytes(&[62_208, 62_207, 0, 1, 2, 3, 4])).unwrap();
        let w = words();
        let expected = [w[7775], w[0], w[1], w[2], w[3], w[4]].join("-");
        assert_eq!(p, expected);
        assert_eq!(generate(&[0xFF; 4]), None);
        assert_eq!(generate(&bytes(&[65_535; 10])), None);
        // A trailing odd byte is ignored.
        assert_eq!(generate(&[1u8; 11]), None);
        assert_eq!(generate(&[]), None);
    }

    #[test]
    fn normalize_trims_lowercases_and_splits() {
        assert_eq!(normalize("  Apple Banana-cherry "), "apple-banana-cherry");
        assert_eq!(normalize("a--b"), "a-b");
        assert_eq!(normalize(""), "");
    }
}
