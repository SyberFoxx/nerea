//! Durable facts, one small Markdown file per topic.
//!
//! The rolling summary says what happened; a card says what is still true. A
//! card is *edited* where a recalled fragment is only appended, so it survives
//! the turn it came from and the user can open the file and fix it when she is
//! wrong. That difference is the whole feature.
//!
//! Selection is a pure function over the message: tags weigh more than the
//! title, the title more than the body, and nothing above the floor means no
//! card at all. A wrong card is worse than no card.

use std::path::{Path, PathBuf};

use crate::recall::terms;

/// A card is a note, not a document. Past this it stops being readable and
/// starts eating the prompt.
pub const MAX_CARD_BYTES: usize = 2_048;

/// Three cards is the most a single message carries: enough to orient, not
/// enough to crowd out the conversation.
pub const MAX_SELECTED: usize = 3;

/// A tag is what a human chose on purpose; a body word is only a coincidence.
const TAG_WEIGHT: usize = 3;
const TITLE_WEIGHT: usize = 2;
const BODY_WEIGHT: usize = 1;
/// One real hit is a match: a card you cannot recall is a card that does not
/// exist. The limit of three is what bounds the noise, not this floor.
const MIN_SCORE: usize = 1;

/// The whole block, in characters: about 600 tokens, the same order as the
/// summary and never more than the identity.
pub const MAX_BLOCK_CHARS: usize = 2_400;

/// One card, as it lives on disk and in memory.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub topic: String,
    pub title: String,
    pub tags: Vec<String>,
    pub updated: String,
    pub body: String,
}

/// Where the cards live: inside her own data dir, never in the config.
pub fn dir() -> PathBuf {
    crate::paths::data_dir().join("cards")
}

/// What `remember` did, so the answer can say it instead of guessing.
#[derive(Debug, Clone, PartialEq)]
pub enum Kept {
    Created { path: PathBuf, facts: usize },
    Updated { path: PathBuf, facts: usize },
    AlreadyThere { path: PathBuf },
}

/// Every readable card in `dir`, ordered by topic so the result is stable.
pub fn load(dir: &Path) -> Vec<Card> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut cards: Vec<Card> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        })
        .filter_map(|path| load_at(&path))
        .collect();

    cards.sort_by(|left, right| left.topic.cmp(&right.topic));
    cards
}

/// One card from one file, if the file is a card at all.
pub fn load_at(path: &Path) -> Option<Card> {
    let text = std::fs::read_to_string(path).ok()?;
    let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
    parse(&text, stem)
}

/// Front matter, a blank line, then the body. Unknown keys are ignored and a
/// file with no front matter at all is not a card: a stray note in the folder
/// must not become memory.
pub fn parse(text: &str, fallback_topic: &str) -> Option<Card> {
    let lines: Vec<&str> = text.lines().collect();
    let mut topic: Option<String> = None;
    let mut title: Option<String> = None;
    let mut tags: Vec<String> = Vec::new();
    let mut updated: Option<String> = None;
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index].trim();
        if line.is_empty() {
            index += 1;
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            break;
        };
        match key.trim().to_lowercase().as_str() {
            "topic" => topic = Some(value.trim().to_owned()),
            "title" => title = Some(value.trim().to_owned()),
            "tags" => {
                tags = value
                    .split(',')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            "updated" => updated = Some(value.trim().to_owned()),
            _ => {}
        }
        index += 1;
    }

    if updated.is_none() && title.is_none() && tags.is_empty() {
        return None;
    }

    let topic = topic
        .filter(|topic| !topic.trim().is_empty())
        .unwrap_or_else(|| fallback_topic.trim().to_owned());
    if topic.is_empty() {
        return None;
    }

    let body = lines[index.min(lines.len())..].join("\n").trim().to_owned();
    let title = title.filter(|title| !title.trim().is_empty()).unwrap_or_else(|| topic.clone());

    Some(Card { topic, title, tags, updated: updated.unwrap_or_default(), body })
}

/// The file that holds this card, in the shape it is written to disk.
pub fn render(card: &Card) -> String {
    format!(
        "topic: {}\ntitle: {}\ntags: {}\nupdated: {}\n\n{}\n",
        card.topic,
        card.title,
        card.tags.join(", "),
        card.updated,
        card.body.trim()
    )
}

/// The cards that earn their tokens for this message, best first.
///
/// Only a card that shares at least one word with the message is considered:
/// most messages share nothing with most cards, which is why the common turn
/// injects nothing at all.
pub fn select(cards: &[Card], message: &str, limit: usize) -> Vec<Card> {
    let mut query: Vec<String> = Vec::new();
    for term in terms(message) {
        if !query.contains(&term) {
            query.push(term);
        }
    }
    if query.is_empty() || cards.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(usize, &Card)> = cards
        .iter()
        .filter_map(|card| {
            let tags: Vec<String> = card.tags.iter().flat_map(|tag| terms(tag)).collect();
            let title = terms(&card.title);
            let body = terms(&card.body);

            let mut score = 0usize;
            for term in &query {
                if tags.contains(term) {
                    score += TAG_WEIGHT;
                } else if title.contains(term) {
                    score += TITLE_WEIGHT;
                } else if body.contains(term) {
                    score += BODY_WEIGHT;
                }
            }

            (score >= MIN_SCORE).then_some((score, card))
        })
        .collect();

    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.topic.cmp(&right.1.topic)));
    scored.into_iter().take(limit).map(|(_, card)| card.clone()).collect()
}

/// The prompt block, or nothing at all when there is nothing to say.
///
/// Nothing is injected on the common turn, which is the point: a message that
/// matches no card costs no tokens.
pub fn block(cards: &[Card]) -> Option<String> {
    if cards.is_empty() {
        return None;
    }

    let mut out = String::new();
    for card in cards {
        let room = MAX_BLOCK_CHARS.saturating_sub(out.chars().count());
        if room < 200 {
            break;
        }

        let title = if card.title.trim().is_empty() { card.topic.as_str() } else { card.title.as_str() };
        let piece = format!("### {title} ({})\n{}\n", card.topic, card.body.trim());
        out.extend(piece.chars().take(room));
    }

    let trimmed = out.trim_end().to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Writes one fact into its topic's card, creating the card when it is new.
///
/// It never deletes and never truncates: a fact that does not fit is refused
/// with a reason, because a silently dropped memory is worse than a full card.
pub fn remember(
    dir: &Path,
    topic: &str,
    title: Option<&str>,
    tags: &str,
    fact: &str,
) -> Result<Kept, String> {
    let topic = slug(topic)?;
    let fact = fact.trim();
    if fact.is_empty() {
        return Err("no hay nada que recordar".to_owned());
    }

    let path = dir.join(format!("{topic}.md"));
    let existing = path.is_file().then(|| load_at(&path)).flatten();
    let incoming: Vec<String> = tags
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
        .collect();

    let (card, created) = match existing {
        Some(mut card) => {
            // Remembering the same thing twice is not an error, it is a no-op:
            // the card is a set of facts, not a log.
            let already = card
                .body
                .lines()
                .any(|line| line.trim_start_matches("- ").trim().eq_ignore_ascii_case(fact));
            if already {
                return Ok(Kept::AlreadyThere { path });
            }

            if !card.body.trim().is_empty() {
                card.body = format!("{}\n- {fact}", card.body.trim_end());
            } else {
                card.body = format!("- {fact}");
            }
            card.updated = today();
            for tag in incoming {
                if !card.tags.contains(&tag) {
                    card.tags.push(tag);
                }
            }
            (card, false)
        }
        None => {
            let title = title
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| topic.replace('_', " "));
            // No tags means the topic is the only handle, so it becomes one:
            // without this a fact written by `remember` could never be found.
            let tags = if incoming.is_empty() { terms(&topic) } else { incoming };
            let card = Card {
                topic: topic.clone(),
                title,
                tags,
                updated: today(),
                body: format!("- {fact}"),
            };
            (card, true)
        }
    };

    let text = render(&card);
    if text.len() > MAX_CARD_BYTES {
        return Err(format!(
            "la tarjeta «{topic}» ya está llena: el hecho no entra en {MAX_CARD_BYTES} bytes. \
Guardalo en otro tema, o pedime que resuma esta tarjeta primero"
        ));
    }

    std::fs::create_dir_all(dir)
        .map_err(|err| format!("no puedo crear la carpeta de tarjetas: {err}"))?;

    // Written beside the card and renamed: a card is never half written.
    let temp = path.with_extension("md.tmp");
    std::fs::write(&temp, &text).map_err(|err| format!("no pude escribir la tarjeta: {err}"))?;
    std::fs::rename(&temp, &path).map_err(|err| format!("no pude guardar la tarjeta: {err}"))?;

    let facts = card
        .body
        .lines()
        .filter(|line| line.trim_start().starts_with("- "))
        .count();

    Ok(if created { Kept::Created { path, facts } } else { Kept::Updated { path, facts } })
}

/// A topic is one word, never a path: the file is built from it.
fn slug(input: &str) -> Result<String, String> {
    let trimmed = input.trim().to_lowercase();
    if trimmed.is_empty() {
        return Err("falta el tema de la tarjeta".to_owned());
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        return Err("el tema tiene que ser una sola palabra, no una ruta".to_owned());
    }

    let cleaned: String = trimmed
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '_' || character == '-' || character == ' ' {
                character
            } else {
                '_'
            }
        })
        .collect();

    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join("_");
    if cleaned.is_empty() {
        return Err("el tema tiene que tener letras o números".to_owned());
    }
    Ok(cleaned.chars().take(60).collect())
}

/// Today in UTC, as `YYYY-MM-DD`. Civil dates by hand: one date field does not
/// justify a dependency.
fn today() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    civil_date((seconds / 86_400) as i64)
}

/// Days since the epoch to `YYYY-MM-DD`. Howard Hinnant's algorithm: exact for
/// every date the epoch arithmetic can hold, and no dependency for one field.
fn civil_date(days: i64) -> String {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 { month_position + 3 } else { month_position - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nerea-cards-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn card(topic: &str, title: &str, tags: &str, body: &str) -> Card {
        Card {
            topic: topic.to_owned(),
            title: title.to_owned(),
            tags: tags.split(',').map(|tag| tag.trim().to_owned()).filter(|tag| !tag.is_empty()).collect(),
            updated: "2026-11-25".to_owned(),
            body: body.to_owned(),
        }
    }

    fn library() -> Vec<Card> {
        vec![
            card(
                "lore_novela",
                "La novela de Nerea",
                "novela, nerea, capitulos",
                "- Nerea vive en el faro\n- El capítulo 2 arranca con la tormenta",
            ),
            card(
                "proyectos_actuales",
                "En qué está trabajando Lioner",
                "proyectos, nerea, rust",
                "- Nerea, la compañera de escritorio en Rust",
            ),
            card(
                "eventos_clave",
                "Fechas que importan",
                "fechas, aniversarios",
                "- El 3 de marzo empezó todo",
            ),
        ]
    }

    #[test]
    fn a_card_round_trips_through_its_file() {
        let original = card("lore_novela", "La novela", "novela, escritura", "- primer hecho");
        let text = render(&original);
        let parsed = parse(&text, "otro").expect("a card");

        assert_eq!(parsed, original, "{text}");
    }

    #[test]
    fn a_broken_card_is_skipped_not_fatal() {
        let dir = scratch("broken");
        std::fs::write(dir.join("roto.md"), "esto no tiene front matter").expect("write");
        std::fs::write(dir.join("buena.md"), render(&card("buena", "Buena", "x", "- hecho")))
            .expect("write");

        let loaded = load(&dir);

        assert_eq!(loaded.len(), 1, "{loaded:?}");
        assert_eq!(loaded[0].topic, "buena");
    }

    #[test]
    fn selection_pulls_the_novel_card_and_not_the_projects_one() {
        let cards = library();
        let picked = select(&cards, "¿en qué capítulo va la novela?", 3);
        let topics: Vec<&str> = picked.iter().map(|card| card.topic.as_str()).collect();

        assert!(topics.contains(&"lore_novela"), "{topics:?}");
        assert!(!topics.contains(&"proyectos_actuales"), "{topics:?}");
    }

    #[test]
    fn a_greeting_injects_nothing() {
        let cards = library();
        assert!(select(&cards, "hola", 3).is_empty());
        assert!(select(&cards, "   ", 3).is_empty());
    }

    #[test]
    fn tags_beat_title_and_title_beats_body() {
        let cards = vec![
            card("por_cuerpo", "Otra cosa", "nada", "- menciona la novela al pasar"),
            card("por_titulo", "La novela", "nada", "- nada"),
            card("por_tag", "Otra cosa", "novela", "- nada"),
        ];

        let picked = select(&cards, "novela", 3);
        let topics: Vec<&str> = picked.iter().map(|card| card.topic.as_str()).collect();

        assert_eq!(topics, vec!["por_tag", "por_titulo", "por_cuerpo"], "{topics:?}");
    }

    /// The floor that matters: a card sharing no word with the message is never
    /// injected, however many cards exist.
    #[test]
    fn a_card_with_no_shared_word_is_never_injected() {
        let cards = library();
        assert!(select(&cards, "receta de empanadas gallegas", 3).is_empty());
    }

    #[test]
    fn selection_is_the_same_every_time() {
        let cards = library();
        let first = select(&cards, "novela", 3);
        let second = select(&cards, "novela", 3);
        assert_eq!(first, second);
    }

    #[test]
    fn the_block_never_exceeds_its_budget() {
        let heavy: Vec<Card> = (0..3)
            .map(|index| {
                card(
                    &format!("tema_{index}"),
                    &format!("Tema {index}"),
                    "novela",
                    &"x".repeat(1_900),
                )
            })
            .collect();

        let block = block(&heavy).expect("a block");
        assert!(block.chars().count() <= MAX_BLOCK_CHARS, "{}", block.chars().count());
    }

    #[test]
    fn no_cards_means_no_block() {
        assert!(block(&[]).is_none());
    }

    #[test]
    fn remember_creates_a_card_that_survives_a_reload() {
        let dir = scratch("remember-new");
        let kept = remember(&dir, "lore_novela", Some("La novela"), "novela, capitulos", "Nerea vive en el faro")
            .expect("remembered");

        assert!(matches!(kept, Kept::Created { facts: 1, .. }), "{kept:?}");

        let loaded = load(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].topic, "lore_novela");
        assert_eq!(loaded[0].title, "La novela");
        assert!(loaded[0].body.contains("Nerea vive en el faro"));
        assert_eq!(loaded[0].tags, vec!["novela", "capitulos"]);
    }

    #[test]
    fn a_card_written_without_tags_is_findable_by_its_topic() {
        let dir = scratch("remember-topic-tag");
        remember(&dir, "cumpleanos", None, "", "es el 12 de abril").expect("remembered");

        let loaded = load(&dir);
        assert_eq!(loaded[0].tags, vec!["cumpleanos"]);

        let picked = select(&loaded, "¿cuándo es mi cumpleaños?", 3);
        assert_eq!(picked.len(), 1, "{picked:?}");
    }

    #[test]
    fn remember_appends_without_duplicating() {
        let dir = scratch("remember-dupe");
        remember(&dir, "hechos", None, "", "El faro está en la costa").expect("first");
        let again = remember(&dir, "hechos", None, "", "el faro está en la costa").expect("again");

        assert!(matches!(again, Kept::AlreadyThere { .. }), "{again:?}");
        let loaded = load(&dir);
        assert_eq!(loaded[0].body.matches("faro").count(), 1, "{}", loaded[0].body);
    }

    #[test]
    fn remember_never_drops_an_existing_fact() {
        let dir = scratch("remember-keeps");
        remember(&dir, "hechos", None, "", "El faro está en la costa").expect("first");
        remember(&dir, "hechos", None, "", "Lioner escribe de noche").expect("second");

        let loaded = load(&dir);
        assert!(loaded[0].body.contains("El faro está en la costa"), "{}", loaded[0].body);
        assert!(loaded[0].body.contains("Lioner escribe de noche"), "{}", loaded[0].body);
    }

    #[test]
    fn a_card_that_would_pass_two_kilobytes_is_refused() {
        let dir = scratch("remember-full");
        let long = "y".repeat(1_200);
        let other = format!("z{}", "w".repeat(1_200));
        remember(&dir, "hechos", None, "", &long).expect("first half");

        let err = remember(&dir, "hechos", None, "", &other).expect_err("must refuse");
        assert!(err.contains("llena"), "{err}");

        // What was there is still there, whole.
        let loaded = load(&dir);
        assert!(loaded[0].body.contains(&long), "the first fact survived");
    }

    #[test]
    fn a_topic_cannot_be_a_path() {
        let dir = scratch("remember-topic");
        for bad in ["../evil", "a/b", "", "   "] {
            let err = remember(&dir, bad, None, "", "un hecho").expect_err("must refuse");
            assert!(!err.is_empty(), "{bad:?}");
        }
    }

    #[test]
    fn civil_dates_line_up_with_the_calendar() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_782), "2026-11-25");
        assert_eq!(civil_date(-1), "1969-12-31");
    }
}
