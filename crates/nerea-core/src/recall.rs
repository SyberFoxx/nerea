//! Lexical recall over the transcript.
//!
//! The summary carries the gist; this finds the specific episode. A small BM25
//! index built from the JSONL at runtime, with no model download and no
//! database: the transcript stays the only source of truth and the index is
//! rebuilt when it grows.
//!
//! Lexical, not semantic, on purpose. It will miss a paraphrase, and it never
//! invents a memory: if the words are not there, nothing comes back.

use std::collections::HashMap;

use crate::ai::Role;
use crate::transcript::Entry;

const K1: f64 = 1.2;
const B: f64 = 0.75;

/// Words too common to carry meaning in a lookup.
const STOPWORDS: &[&str] = &[
    "que", "como", "para", "por", "con", "los", "las", "una", "uno", "unos", "unas", "del", "los",
    "mas", "pero", "sus", "esta", "este", "esto", "esos", "esas", "hay", "muy", "sin", "sobre",
    "entre", "cuando", "donde", "porque", "tambien", "todo", "toda", "todos", "todas", "algo",
    "algun", "alguna", "ser", "son", "fue", "era", "he", "has", "ha", "han", "es", "estoy",
    "estas", "esta", "estamos", "the", "and", "for", "that", "with", "you", "your", "was", "were",
];

/// Lowercase and fold the Spanish accents, so `canción` finds `cancion`.
fn fold(text: &str) -> String {
    text.chars()
        .flat_map(|character| match character.to_lowercase().next() {
            Some('á') => vec!['a'],
            Some('é') => vec!['e'],
            Some('í') => vec!['i'],
            Some('ó') => vec!['o'],
            Some('ú') | Some('ü') => vec!['u'],
            Some('ñ') => vec!['n'],
            Some(other) => vec![other],
            None => vec![],
        })
        .collect()
}

/// The terms worth indexing: three characters or more, and not a stopword.
pub fn terms(text: &str) -> Vec<String> {
    fold(text)
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .filter(|word| !STOPWORDS.contains(word))
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Default)]
struct Document {
    /// Position in the entry list this index was built from.
    position: usize,
    length: usize,
    counts: HashMap<String, usize>,
}

/// One entry that matches, with its score.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub position: usize,
    pub score: f64,
}

#[derive(Debug, Default)]
pub struct Corpus {
    documents: Vec<Document>,
    frequencies: HashMap<String, usize>,
    average_length: f64,
}

impl Corpus {
    /// Indexes the conversation turns; system entries carry no recall value.
    pub fn build(entries: &[Entry]) -> Self {
        let mut documents = Vec::new();
        let mut frequencies: HashMap<String, usize> = HashMap::new();
        let mut total = 0usize;

        for (position, entry) in entries.iter().enumerate() {
            if entry.role == Role::System {
                continue;
            }

            let words = terms(&entry.content);
            if words.is_empty() {
                continue;
            }

            let mut counts: HashMap<String, usize> = HashMap::new();
            for word in &words {
                *counts.entry(word.clone()).or_insert(0) += 1;
            }
            for word in counts.keys() {
                *frequencies.entry(word.clone()).or_insert(0) += 1;
            }

            total += words.len();
            documents.push(Document { position, length: words.len(), counts });
        }

        let average_length = if documents.is_empty() {
            0.0
        } else {
            total as f64 / documents.len() as f64
        };

        Self { documents, frequencies, average_length }
    }

    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// Best matches first. Ties go to the older entry, which is the one the
    /// summary is least likely to already carry.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let terms = terms(query);
        if terms.is_empty() || self.documents.is_empty() {
            return Vec::new();
        }

        let total = self.documents.len() as f64;
        let mut scored: Vec<Hit> = Vec::new();

        for document in &self.documents {
            let mut score = 0.0;

            for term in &terms {
                let Some(count) = document.counts.get(term) else { continue };
                let frequency = *self.frequencies.get(term).unwrap_or(&0) as f64;
                if frequency == 0.0 {
                    continue;
                }

                let idf = ((total - frequency + 0.5) / (frequency + 0.5) + 1.0).ln();
                let length = document.length as f64;
                let norm = K1 * (1.0 - B + B * length / self.average_length.max(1.0));
                let hits = *count as f64;

                score += idf * (hits * (K1 + 1.0)) / (hits + norm);
            }

            if score > 0.0 {
                scored.push(Hit { position: document.position, score });
            }
        }

        scored.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(left.position.cmp(&right.position))
        });
        scored.truncate(limit);
        scored
    }
}

/// Human label for how old an entry is, relative to `now`.
pub fn age_label(at: u64, now: u64) -> String {
    let days = now.saturating_sub(at) / 86_400_000;

    match days {
        0 => "hoy".to_owned(),
        1 => "ayer".to_owned(),
        days if days < 30 => format!("hace {days} días"),
        days if days < 60 => "hace 1 mes".to_owned(),
        days if days < 365 => format!("hace {} meses", days / 30),
        days if days < 730 => "hace 1 año".to_owned(),
        days => format!("hace {} años", days / 365),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(role: Role, content: &str) -> Entry {
        Entry { role, content: content.to_owned(), at: 0 }
    }

    fn corpus() -> (Vec<Entry>, Corpus) {
        let entries = vec![
            entry(Role::User, "Hoy configuramos el keyring para las claves de la API."),
            entry(Role::Assistant, "Nada de claves en texto plano, mi dragón."),
            entry(Role::User, "Después probamos el avatar con la voz clonada en japonés."),
            entry(Role::Assistant, "La voz suena bien, aunque tarda quince segundos."),
            entry(Role::User, "Mañana revisamos el empaquetado en GitHub Actions."),
        ];
        let corpus = Corpus::build(&entries);
        (entries, corpus)
    }

    #[test]
    fn finds_the_episode_that_mentions_the_words() {
        let (_, corpus) = corpus();
        let hits = corpus.search("dónde guardamos las claves", 3);

        assert!(!hits.is_empty());
        assert_eq!(hits[0].position, 0, "the keyring turn wins");
    }

    #[test]
    fn accents_do_not_hide_a_match() {
        let (_, corpus) = corpus();
        // The transcript has "japonés"; the query has no accent.
        let hits = corpus.search("japones", 3);
        assert!(hits.iter().any(|hit| hit.position == 2));
    }

    #[test]
    fn stopwords_alone_find_nothing() {
        let (_, corpus) = corpus();
        assert!(corpus.search("que es lo que", 3).is_empty());
        assert!(corpus.search("   ", 3).is_empty());
    }

    #[test]
    fn an_unknown_topic_returns_nothing_instead_of_noise() {
        let (_, corpus) = corpus();
        assert!(corpus.search("receta de empanadas gallegas", 3).is_empty());
    }

    #[test]
    fn the_limit_is_respected_and_scores_descend() {
        let (_, corpus) = corpus();
        let hits = corpus.search("voz empaquetado clave avatar", 2);
        assert!(hits.len() <= 2);
        assert!(hits.windows(2).all(|pair| pair[0].score >= pair[1].score));
    }

    #[test]
    fn ages_read_like_someone_talking() {
        let day = 86_400_000u64;
        let now = 1_000 * day;
        assert_eq!(age_label(now, now), "hoy");
        assert_eq!(age_label(now - day, now), "ayer");
        assert_eq!(age_label(now - 5 * day, now), "hace 5 días");
        assert_eq!(age_label(now - 45 * day, now), "hace 1 mes");
        assert_eq!(age_label(now - 90 * day, now), "hace 3 meses");
        assert_eq!(age_label(now - 500 * day, now), "hace 1 año");
        assert_eq!(age_label(now - 900 * day, now), "hace 2 años");
    }

    #[test]
    fn an_empty_transcript_is_harmless() {
        let corpus = Corpus::build(&[]);
        assert!(corpus.is_empty());
        assert!(corpus.search("hola", 3).is_empty());
    }
}
