//! Names and looks for the account rail: automatic names from sign-in
//! addresses, de-duplication, and the saved icon and colour per account.

use std::collections::HashMap;

use egui::Color32;

use crate::settings::AccountLook;
use crate::theme;

/// Lucide icon names an account can wear (see `theme::Icon::for_account`).
pub const ICONS: [&str; 12] = [
    "briefcase",
    "building-2",
    "house",
    "star",
    "heart",
    "rocket",
    "globe",
    "book-open",
    "coffee",
    "zap",
    "flask-conical",
    "graduation-cap",
];

/// Swatches, in the order of `theme::avatar_color`'s hues.
pub const SWATCHES: [&str; 10] = [
    "#d97a34", "#3f8fd2", "#8e6bd6", "#2ea67f", "#d25b8a", "#c9a227", "#5b8a3a", "#4f6dd1",
    "#c04f45", "#2b9ab0",
];

const CONSUMER: [&str; 5] = ["outlook", "hotmail", "live", "msn", "passport"];
const SUFFIX_FIRST: [&str; 7] = ["co", "com", "org", "net", "ac", "gov", "edu"];

/// A short name from the domain of a sign-in name.
pub fn auto_name(username: &str) -> String {
    let Some((_, domain)) = username.rsplit_once('@') else {
        return "Account".into();
    };
    let domain = domain.trim().to_lowercase();
    let label = if let Some(rest) = domain.strip_suffix(".onmicrosoft.com") {
        rest.split('.').next().unwrap_or("").to_string()
    } else {
        let mut labels: Vec<&str> = domain.split('.').filter(|l| !l.is_empty()).collect();
        let n = labels.len();
        if n >= 3 && SUFFIX_FIRST.contains(&labels[n - 2]) && labels[n - 1].chars().count() == 2 {
            labels.truncate(n - 2);
        } else if n >= 2 {
            labels.truncate(n - 1);
        }
        labels.last().copied().unwrap_or("").to_string()
    };
    if label.is_empty() {
        return "Account".into();
    }
    if CONSUMER.contains(&label.as_str()) {
        return "Personal".into();
    }
    let mut chars = label.chars();
    let first: String = chars
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    first + chars.as_str()
}

/// One display name per account: the custom name when not blank, else the
/// automatic one; repeats get " 2", " 3" in order.
pub fn names(usernames: &[String], custom: &[Option<String>]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    usernames
        .iter()
        .enumerate()
        .map(|(i, user)| {
            let base = custom
                .get(i)
                .and_then(|c| c.as_deref())
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| auto_name(user));
            let count = seen.entry(base.clone()).or_insert(0);
            *count += 1;
            if *count == 1 {
                base
            } else {
                format!("{base} {count}")
            }
        })
        .collect()
}

/// Mark text for an account name: first and last word's first letters in
/// capitals ("Contoso Labs" is "CL"); a single word gives its first letter
/// capital and its second lower ("Northwind" is "No"). Whole graphemes only.
pub fn initials(name: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == ',' || c == '(')
        .filter(|w| w.chars().next().is_some_and(char::is_alphanumeric))
        .collect();
    match words.as_slice() {
        [] => "?".into(),
        [only] => {
            let mut g = only.graphemes(true);
            let first = g.next().unwrap_or_default().to_uppercase();
            let second = g.next().unwrap_or_default().to_lowercase();
            first + &second
        }
        _ => theme::initials(name),
    }
}

/// What the rail draws for one account.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub name: String,
    pub initials: String,
    pub icon: Option<String>,
    pub color: Color32,
}

fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Combine the display name with the saved icon and colour; unknown icons and
/// bad colours fall back to no icon and the automatic colour.
pub fn resolve(id: &str, name: String, saved: Option<&AccountLook>, dark: bool) -> Look {
    let icon = saved
        .and_then(|s| s.icon.as_deref())
        .filter(|i| ICONS.contains(i))
        .map(str::to_string);
    let color = saved
        .and_then(|s| s.color.as_deref())
        .and_then(parse_hex)
        .unwrap_or_else(|| theme::avatar_color(id, dark));
    Look {
        initials: initials(&name),
        name,
        icon,
        color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_name_rules() {
        assert_eq!(auto_name("alex"), "Account");
        assert_eq!(auto_name("alex@contoso.com"), "Contoso");
        assert_eq!(auto_name("x@contoso.onmicrosoft.com"), "Contoso");
        assert_eq!(auto_name("me@outlook.com"), "Personal");
        assert_eq!(auto_name("a@MAIL.Contoso.co.uk"), "Contoso");
        assert_eq!(auto_name("a@hotmail.co.uk"), "Personal");
        assert_eq!(auto_name("a@live.com"), "Personal");
        assert_eq!(auto_name("a@msn.com"), "Personal");
        assert_eq!(auto_name("a@wide-world.com"), "Wide-world");
    }

    #[test]
    fn names_third_clash() {
        let u: Vec<String> = ["a@x.com", "b@x.com", "c@x.com"].map(String::from).into();
        assert_eq!(names(&u, &[None, None, None]), ["X", "X 2", "X 3"]);
        let blank = [Some("  ".to_string()), None, None];
        assert_eq!(names(&u, &blank), ["X", "X 2", "X 3"]);
        assert_eq!(
            names(&["a@contoso.com".to_string()], &[Some("Work".into())]),
            ["Work"]
        );
        let two: Vec<String> = ["a@contoso.com", "b@contoso.com"].map(String::from).into();
        assert_eq!(names(&two, &[None, None]), ["Contoso", "Contoso 2"]);
    }

    #[test]
    fn resolve_bad_hex_falls_back() {
        let auto = theme::avatar_color("o.t", true);
        let bad = AccountLook {
            icon: Some("nope".into()),
            color: Some("zzz".into()),
            ..Default::default()
        };
        let l = resolve("o.t", "Contoso".into(), Some(&bad), true);
        assert_eq!(l.icon, None);
        assert_eq!(l.color, auto);
        let none = resolve("o.t", "Contoso".into(), None, true);
        assert_eq!(none.initials, "Co");
        assert_eq!(none.color, auto);
        let good = AccountLook {
            icon: Some("star".into()),
            color: Some("#3f8fd2".into()),
            ..Default::default()
        };
        let l = resolve("o.t", "Contoso".into(), Some(&good), true);
        assert_eq!(l.icon.as_deref(), Some("star"));
        assert_eq!(l.color, Color32::from_rgb(0x3f, 0x8f, 0xd2));
    }

    #[test]
    fn initials_one_word_two_letters() {
        assert_eq!(initials("Ä"), "Ä");
        assert_eq!(initials("fabrikam"), "Fa");
        assert_eq!(initials("Northwind"), "No");
        assert_eq!(initials("éclair"), "Éc");
    }

    #[test]
    fn initials_many_words() {
        assert_eq!(initials("Contoso Labs"), "CL");
        assert_eq!(initials("a b c"), "AC");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn non_ascii_initials() {
        let l = resolve("o.t", "Émile Zola".into(), None, false);
        assert_eq!(l.initials, "ÉZ");
    }

    #[test]
    fn auto_name_non_ascii_and_idn() {
        assert_eq!(auto_name("a@bücher.de"), "Bücher");
        // Punycode is not decoded: the label is shown as typed, capitalised.
        assert_eq!(auto_name("a@xn--bcher-kva.de"), "Xn--bcher-kva");
        // Uppercase non-ASCII is lowercased whole, then the first is capitalised.
        assert_eq!(auto_name("a@ÄÖ.com"), "Äö");
        assert_eq!(initials(&auto_name("a@ÄÖ.com")), "Äö");
        assert_eq!(initials("ÄÖ"), "Äö");
    }

    #[test]
    fn names_clash_suffix_non_ascii() {
        let u: Vec<String> = ["a@bücher.de", "b@BÜCHER.de", "c@bücher.com"]
            .map(String::from)
            .into();
        assert_eq!(
            names(&u, &[None, None, None]),
            ["Bücher", "Bücher 2", "Bücher 3"]
        );
        let custom = [Some("Ärzte".to_string()), Some(" Ärzte ".to_string())];
        assert_eq!(names(&u[..2], &custom), ["Ärzte", "Ärzte 2"]);
    }
}
