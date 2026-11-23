use std::{
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

/// The phrase bank is text: it is embedded so a running build never depends on
/// the working directory or on the machine-local audio folder.
const BANK: &str = include_str!("../../../voice/bank.json");

#[derive(Debug, Clone, Deserialize)]
pub struct Phrase {
    pub id: String,
    pub category: String,
    pub ja: String,
    pub es: String,
}

#[derive(Debug, Deserialize)]
struct Bank {
    #[serde(default)]
    entries: Vec<Phrase>,
}

#[derive(Default)]
pub struct PhraseBank {
    entries: Vec<Phrase>,
    last: Mutex<Option<usize>>,
}

impl PhraseBank {
    pub fn load() -> Result<Self, String> {
        let bank: Bank = serde_json::from_str(BANK).map_err(|err| err.to_string())?;
        Ok(Self { entries: bank.entries, last: Mutex::new(None) })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn categories(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.entries.iter().map(|entry| entry.category.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// A random line from the category, never the same one twice in a row.
    pub fn pick(&self, category: &str) -> Option<Phrase> {
        let matches: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.category == category)
            .map(|(index, _)| index)
            .collect();

        if matches.is_empty() {
            return None;
        }

        let previous = *self.last.lock().ok()?;
        let choice = if matches.len() == 1 {
            0
        } else {
            let mut index = random(matches.len());
            if Some(matches[index]) == previous {
                index = (index + 1) % matches.len();
            }
            index
        };

        let picked = matches[choice];
        if let Ok(mut last) = self.last.lock() {
            *last = Some(picked);
        }

        self.entries.get(picked).cloned()
    }
}

/// Small xorshift: enough variety for picking lines, no dependency needed.
fn random(bound: usize) -> usize {
    static SEED: AtomicU64 = AtomicU64::new(0);

    let mut state = SEED.load(Ordering::Relaxed);
    if state == 0 {
        state = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
    }

    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    SEED.store(state, Ordering::Relaxed);

    (state as usize) % bound.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_bank_parses() {
        let bank = PhraseBank::load().expect("embedded bank");
        assert!(bank.len() >= 60, "bank has {} lines", bank.len());
    }

    #[test]
    fn every_expected_category_exists() {
        let bank = PhraseBank::load().expect("embedded bank");
        let categories = bank.categories();
        for expected in [
            "boot", "timeofday", "idle", "back", "thinking", "micro", "agree", "surprise", "laugh",
            "tease", "flirt", "jealous", "affection", "complain", "goodbye",
        ] {
            assert!(categories.contains(&expected), "missing category {expected}");
        }
    }

    #[test]
    fn picking_never_repeats_the_previous_line() {
        let bank = PhraseBank::load().expect("embedded bank");
        let first = bank.pick("tease").expect("tease line");
        let second = bank.pick("tease").expect("tease line");
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn unknown_category_returns_nothing() {
        let bank = PhraseBank::load().expect("embedded bank");
        assert!(bank.pick("nope").is_none());
    }
}
