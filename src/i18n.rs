//! Interface language: English or Russian, taken from the language of the
//! Windows interface unless chosen in the About window.
//!
//! Strings stay next to the code that shows them, both languages together:
//! `tr!("Open", "Открыть")` gives the one of the current language (and
//! evaluates only that one, so `format!` arms cost nothing extra). A
//! missing translation does not compile. Strings are built every frame,
//! so nothing caches translated text. Log messages, command line output
//! and technical errors stay English.

use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    En,
    Ru,
}

/// The language as kept in the settings: a fixed one, or the system's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LangChoice {
    #[default]
    System,
    En,
    Ru,
}

impl LangChoice {
    pub const ALL: [LangChoice; 3] = [LangChoice::System, LangChoice::En, LangChoice::Ru];

    pub fn resolve(self) -> Lang {
        match self {
            LangChoice::System => system_lang(),
            LangChoice::En => Lang::En,
            LangChoice::Ru => Lang::Ru,
        }
    }

    /// As listed in the About window: the languages in their own language,
    /// so that they can be found whatever the interface shows.
    pub fn label(self) -> String {
        let own = |lang| match lang {
            Lang::En => "English",
            Lang::Ru => "Русский",
        };
        match self {
            LangChoice::System => {
                let own = own(system_lang());
                crate::tr!(format!("As in Windows ({own})"), format!("Как в Windows ({own})"))
            }
            LangChoice::En | LangChoice::Ru => own(self.resolve()).into(),
        }
    }

}

static LANG: AtomicU8 = AtomicU8::new(0);

/// The language the interface is shown in now.
pub fn lang() -> Lang {
    match LANG.load(Relaxed) {
        1 => Lang::Ru,
        _ => Lang::En,
    }
}

pub fn set_lang(lang: Lang) {
    LANG.store(lang as u8, Relaxed);
}

/// Russian when Windows shows its interface in Russian, else English.
#[cfg(windows)]
pub fn system_lang() -> Lang {
    const LANG_RUSSIAN: u16 = 0x19;
    let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    if id & 0x3FF == LANG_RUSSIAN { Lang::Ru } else { Lang::En }
}

/// Russian when the locale (`LC_ALL`, `LC_MESSAGES` or `LANG`) is, else
/// English.
#[cfg(not(windows))]
pub fn system_lang() -> Lang {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    if locale.starts_with("ru") { Lang::Ru } else { Lang::En }
}

/// The English or the Russian expression, by the current language.
#[macro_export]
macro_rules! tr {
    ($en:expr, $ru:expr $(,)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::En => $en,
            $crate::i18n::Lang::Ru => $ru,
        }
    };
}

/// The Russian form of a word for `n` things: `one` for 1, 21, 101…,
/// `few` for 2–4, 22–24…, `many` for 0, 5–20, 25–30… (11–14 included).
pub fn ru_plural<'a>(n: u64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    match (n % 10, n % 100) {
        (_, 11..=14) => many,
        (1, _) => one,
        (2..=4, _) => few,
        _ => many,
    }
}

/// `n` with the word for it in the current language: `1 file`, `3 files`,
/// `3 файла`, `5 файлов`.
#[allow(dead_code)]
pub fn count(n: u64, en: [&str; 2], ru: [&str; 3]) -> String {
    count_in(lang(), n, en, ru)
}

/// `count` for a given language: tests must not rely on the global one.
pub fn count_in(lang: Lang, n: u64, en: [&str; 2], ru: [&str; 3]) -> String {
    let word = match lang {
        Lang::En => {
            if n == 1 { en[0] } else { en[1] }
        }
        Lang::Ru => ru_plural(n, ru[0], ru[1], ru[2]),
    };
    format!("{n} {word}")
}

/// `count` for files.
#[allow(dead_code)]
pub fn files(n: u64) -> String {
    count(n, ["file", "files"], ["файл", "файла", "файлов"])
}

/// `count` for folders.
#[allow(dead_code)]
pub fn folders(n: u64) -> String {
    count(n, ["folder", "folders"], ["папка", "папки", "папок"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_plurals() {
        let f = |n| ru_plural(n, "файл", "файла", "файлов");
        assert_eq!(f(1), "файл");
        assert_eq!(f(2), "файла");
        assert_eq!(f(5), "файлов");
        assert_eq!(f(11), "файлов");
        assert_eq!(f(14), "файлов");
        assert_eq!(f(21), "файл");
        assert_eq!(f(22), "файла");
        assert_eq!(f(111), "файлов");
        assert_eq!(f(0), "файлов");
    }

    #[test]
    fn counts_in_both_languages() {
        assert_eq!(count_in(Lang::En, 1, ["file", "files"], ["а", "б", "в"]), "1 file");
        assert_eq!(count_in(Lang::En, 2, ["file", "files"], ["а", "б", "в"]), "2 files");
        assert_eq!(count_in(Lang::Ru, 3, ["file", "files"], ["файл", "файла", "файлов"]), "3 файла");
    }

    #[test]
    fn choice_is_stored_by_name() {
        // In the settings file as `system`, `en`, `ru`.
        for choice in LangChoice::ALL {
            let json = serde_json::to_string(&choice).unwrap();
            assert_eq!(serde_json::from_str::<LangChoice>(&json).unwrap(), choice);
        }
        assert_eq!(serde_json::to_string(&LangChoice::Ru).unwrap(), "\"ru\"");
        assert!(serde_json::from_str::<LangChoice>("\"de\"").is_err());
        assert_eq!(LangChoice::Ru.label(), "Русский");
    }
}
