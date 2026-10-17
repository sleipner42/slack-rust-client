//! English and Swedish spell checking with the bundled Hunspell dictionaries,
//! plus a personal word list. A word is correct if either language knows it.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use spellbook::Dictionary;

pub struct SpellChecker {
    english: Dictionary,
    swedish: Dictionary,
    personal: Mutex<HashSet<String>>,
    ignored: Mutex<HashSet<String>>,
    cache: Mutex<HashMap<String, bool>>,
    personal_path: Option<PathBuf>,
}

static CHECKER: OnceLock<SpellChecker> = OnceLock::new();

/// Parse the dictionaries off the UI thread; checking starts once they are ready.
pub fn start_loading() {
    std::thread::spawn(|| {
        CHECKER.get_or_init(|| SpellChecker::load(personal_path()));
    });
}

/// The checker, once loaded.
pub fn get() -> Option<&'static SpellChecker> {
    CHECKER.get()
}

#[cfg(test)]
pub fn load_now() -> &'static SpellChecker {
    CHECKER.get_or_init(|| SpellChecker::load(None))
}

impl SpellChecker {
    fn load(personal_path: Option<PathBuf>) -> Self {
        let personal = personal_path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|words| {
                words
                    .lines()
                    .map(str::trim)
                    .filter(|word| !word.is_empty())
                    .map(str::to_lowercase)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            english: Dictionary::new(EN_AFF, EN_DIC).expect("bundled English dictionary"),
            swedish: Dictionary::new(SV_AFF, SV_DIC).expect("bundled Swedish dictionary"),
            personal: Mutex::new(personal),
            ignored: Mutex::new(HashSet::new()),
            cache: Mutex::new(HashMap::new()),
            personal_path,
        }
    }

    pub fn check(&self, word: &str) -> bool {
        if let Some(known) = lock(&self.cache).get(word) {
            return *known;
        }
        let lower = word.to_lowercase();
        let known = lock(&self.personal).contains(&lower)
            || lock(&self.ignored).contains(&lower)
            || self.english.check(word)
            || self.swedish.check(word);
        let mut cache = lock(&self.cache);
        if cache.len() > CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(word.to_owned(), known);
        known
    }

    /// Up to five corrections, from the likelier language first.
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let swedish_first = word.chars().any(|ch| "åäöÅÄÖ".contains(ch));
        let (first, second) = if swedish_first {
            (&self.swedish, &self.english)
        } else {
            (&self.english, &self.swedish)
        };
        let mut found = Vec::new();
        first.suggest(word, &mut found);
        let mut more = Vec::new();
        second.suggest(word, &mut more);
        let mut seen = HashSet::new();
        found
            .into_iter()
            .chain(more)
            .filter(|suggestion| seen.insert(suggestion.clone()))
            .take(MAX_SUGGESTIONS)
            .collect()
    }

    /// Remember a word for good, in the personal word list.
    pub fn add(&self, word: &str) {
        let lower = word.to_lowercase();
        if lock(&self.personal).insert(lower.clone())
            && let Some(path) = &self.personal_path
        {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(file, "{lower}");
            }
        }
        lock(&self.cache).clear();
    }

    /// Accept a word until the app restarts.
    pub fn ignore(&self, word: &str) {
        lock(&self.ignored).insert(word.to_lowercase());
        lock(&self.cache).clear();
    }
}

fn personal_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("better-slack")
            .join("dictionary.txt"),
    )
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

static EN_AFF: &str = include_str!("../../assets/dictionaries/en_US.aff");
static EN_DIC: &str = include_str!("../../assets/dictionaries/en_US.dic");
static SV_AFF: &str = include_str!("../../assets/dictionaries/sv_SE.aff");
static SV_DIC: &str = include_str!("../../assets/dictionaries/sv_SE.dic");
const MAX_SUGGESTIONS: usize = 5;
const CACHE_LIMIT: usize = 20_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_english_and_swedish_and_suggests_corrections() {
        let started = std::time::Instant::now();
        let checker = load_now();
        eprintln!("dictionaries loaded in {:?}", started.elapsed());
        for word in [
            "Hello",
            "receive",
            "they're",
            "hej",
            "kanske",
            "Göteborg",
            "sjukhus",
        ] {
            assert!(checker.check(word), "{word} should be known");
        }
        for word in ["recieve", "teh", "hejsan123x", "kansje"] {
            assert!(!checker.check(word), "{word} should be flagged");
        }
        assert!(checker.suggest("recieve").contains(&"receive".to_owned()));
        assert!(checker.suggest("kansje").contains(&"kanske".to_owned()));
        checker.ignore("Flarb");
        assert!(checker.check("flarb"));
    }
}
