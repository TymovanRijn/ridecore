//! Dutch or English. Every user-facing string is written in both languages
//! next to each other, and `t`/`tf!` pick one at run time.

use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Nl,
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Nl];

    pub fn code(self) -> &'static str {
        match self {
            Lang::Nl => "nl",
            Lang::En => "en",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        match code.get(..2)?.to_ascii_lowercase().as_str() {
            "nl" => Some(Lang::Nl),
            "en" => Some(Lang::En),
            _ => None,
        }
    }

    /// The language's own name, for the picker.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Nl => "Nederlands",
            Lang::En => "English",
        }
    }
}

pub fn lang() -> Lang {
    if english() { Lang::En } else { Lang::Nl }
}

pub fn set_lang(lang: Lang) {
    ENGLISH.store(lang == Lang::En, Ordering::Relaxed);
}

pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// The Dutch or the English text.
pub fn t(nl: &'static str, en: &'static str) -> &'static str {
    if english() { en } else { nl }
}

/// `format!` in the current language: `tf!("{n} renners", "{n} riders")`.
/// Both texts take the same arguments.
#[macro_export]
macro_rules! tf {
    ($nl:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::english() {
            format!($en $(, $arg)*)
        } else {
            format!($nl $(, $arg)*)
        }
    };
}

/// "3e" or "3rd".
pub fn ordinal(n: usize) -> String {
    if !english() {
        return format!("{n}e");
    }
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// The language to start in: `RIDECORE_LANG`, else the saved choice, else
/// the system's language (Dutch for Dutch systems, English for the rest).
pub fn startup_lang(saved: &str) -> Lang {
    if let Some(l) = std::env::var("RIDECORE_LANG")
        .ok()
        .and_then(|v| Lang::from_code(&v))
    {
        return l;
    }
    if let Some(l) = Lang::from_code(saved) {
        return l;
    }
    system_lang().unwrap_or(Lang::En)
}

fn system_lang() -> Option<Lang> {
    #[cfg(target_os = "macos")]
    {
        // The first entry of the macOS language list, like "nl-NL".
        let out = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleLanguages"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let first = text.split('"').nth(1)?;
        return Some(Lang::from_code(first).unwrap_or(Lang::En));
    }
    #[allow(unreachable_code)]
    {
        let v = std::env::var("LC_ALL")
            .or_else(|_| std::env::var("LANG"))
            .ok()?;
        Some(Lang::from_code(&v).unwrap_or(Lang::En))
    }
}

/// English names for the countries and regions in the route catalog.
pub fn place(s: &str) -> String {
    if !english() {
        return s.to_string();
    }
    const MAP: &[(&str, &str)] = &[
        ("Kustroute", "Coast road"),
        ("Luik–Bastenaken–Luik", "Liège–Bastogne–Liège"),
        ("Frankrijk", "France"),
        ("Spanje", "Spain"),
        ("Italië", "Italy"),
        ("Oostenrijk", "Austria"),
        ("Nederland", "Netherlands"),
        ("België", "Belgium"),
        ("Vogezen", "Vosges"),
        ("Asturië", "Asturias"),
        ("Lombardije", "Lombardy"),
        ("Dolomieten", "Dolomites"),
        ("Zuid-Tirol", "South Tyrol"),
        ("Zuid-Holland", "South Holland"),
        ("Wallonië", "Wallonia"),
        ("Gegenereerd", "Generated"),
    ];
    let mut out = s.to_string();
    for (nl, en) in MAP {
        out = out.replace(nl, en);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_parse() {
        assert_eq!(Lang::from_code("nl-NL"), Some(Lang::Nl));
        assert_eq!(Lang::from_code("en_GB.UTF-8"), Some(Lang::En));
        assert_eq!(Lang::from_code("de"), None);
        assert_eq!(Lang::from_code(""), None);
    }
}
