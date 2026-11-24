//! Read-only access to the machine: what she may look at, and how.
//!
//! She never gets a shell. Every capability is a named function with a typed
//! argument shape, declared to the model as a tool. The API says when she wants
//! one — that is why there is no JSON protocol in the prompt: a model that says
//! "voy a buscar" and stops cannot happen, because the call is a field, not a
//! sentence. Every call lands in the audit log.

pub mod audit;
pub mod extract;
pub mod guard;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ai::{ToolCall, ToolSpec};

/// She looks, reads what she found, and answers. Three rounds in total: the
/// last one is the answer, so each round before it can ask for files.
pub const MAX_ROUNDS: usize = 3;
/// A model that dumps twenty calls in one breath is guessing, not reading.
pub const MAX_CALLS_PER_ROUND: usize = 4;

const DEFAULT_LINES: usize = 300;
const MAX_LINES: usize = 1_200;
/// One observation, in characters: about 2k tokens, a quarter of the window.
const MAX_OBSERVATION_CHARS: usize = 8_000;
const MAX_ENTRIES: usize = 200;
const MAX_FILES: usize = 4_000;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_DEPTH: usize = 6;
/// Every document has to be unpacked before it can be read, and that is the
/// expensive part. Forty of them is already a lot of parsing for one question.
const MAX_DEEP_FILES: usize = 40;
/// A name is cheap to compare, so a name search may answer with more paths.
const MAX_FOUND: usize = 200;
/// All the reading one message may do. Two full observations already cost about
/// half the context window, and past that the budget cut drops the very question
/// she was answering — she keeps calling tools with no idea what for.
pub const OBSERVATION_BUDGET: usize = 16_000;
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".cache",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
];

/// What she can do, in the shape the API expects.
pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read_file".to_owned(),
            description: "Read a file and return its text: source code, Markdown, JSON, PDF, \
Word (.docx), OpenDocument (.odt), EPUB or HTML. It cannot read images, audio or video. Long files \
come back in windows of lines; use `offset` to continue where the previous answer stopped."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path, or one starting with ~"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "First line to return, 0-based. Default 0"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "How many lines to return. Default 300, max 1200"
                    },
                    "numbers": {
                        "type": "boolean",
                        "description": "Prefix every line with its number, the way grep -n prints it. Default false"
                    }
                },
                "required": ["path"]
            }),
        },
        ToolSpec {
            name: "list_dir".to_owned(),
            description: "List a directory: name, kind and size of each entry.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path, or one starting with ~"
                    }
                },
                "required": ["path"]
            }),
        },
        ToolSpec {
            name: "search".to_owned(),
            description: "Search for a text inside the files of a directory, case-insensitively. \
Returns path:line: text for each match. With `deep: true` it also reads inside PDF, Word (.docx), \
OpenDocument (.odt) and EPUB files — slower, so it is off by default."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Literal text to look for"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in, or one starting with ~"
                    },
                    "deep": {
                        "type": "boolean",
                        "description": "Also read inside PDF, .docx, .odt and EPUB. Default false"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "How many matches to return. Default 40, max 100"
                    }
                },
                "required": ["query", "path"]
            }),
        },
        ToolSpec {
            name: "find".to_owned(),
            description: "Find files and folders by name across a directory tree, \
case-insensitively. Returns one path per line, with a trailing / for a folder. Use this instead \
of listing folder after folder."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Part of the name to look for, case-insensitive"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to walk, or one starting with ~"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "How many paths to return. Default 200, max 400"
                    }
                },
                "required": ["pattern", "path"]
            }),
        },
    ]
}

#[derive(Debug, Clone)]
pub struct ToolOutcome {
    /// Short label for the UI, in her voice.
    pub summary: String,
    /// What the model reads.
    pub observation: String,
    pub ok: bool,
}

/// The arguments as the model wrote them, or a null value if they are not JSON.
fn args_of(call: &ToolCall) -> Value {
    serde_json::from_str(call.arguments.trim()).unwrap_or(Value::Null)
}

/// One line for her subtitle while the tool runs.
pub fn describe(call: &ToolCall) -> String {
    let args = args_of(call);
    let text = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or_default();

    match call.name.as_str() {
        "read_file" => format!("leyendo {}", short_path(text("path"))),
        "list_dir" => format!("mirando {}", short_path(text("path"))),
        "search" => {
            let deep = args.get("deep").and_then(Value::as_bool).unwrap_or(false);
            if deep {
                format!("buscando «{}» adentro de los documentos", cut(text("query"), 40))
            } else {
                format!("buscando «{}»", cut(text("query"), 40))
            }
        }
        "find" => format!("buscando el archivo «{}»", cut(text("pattern"), 40)),
        other => format!("usando {other}"),
    }
}

/// What one message may read, in characters.
pub struct Budget {
    left: usize,
}

impl Budget {
    pub fn per_message() -> Self {
        Self { left: OBSERVATION_BUDGET }
    }

    pub fn left(&self) -> usize {
        self.left
    }
}

/// Runs one call and charges what it returned against the turn.
///
/// Every call gets an answer, refused or not: a call without its result breaks
/// the protocol for every round that follows.
pub fn run_charged(call: &ToolCall, budget: &mut Budget) -> ToolOutcome {
    if budget.left == 0 {
        return ToolOutcome {
            summary: describe(call),
            observation: "No leo más en este turno: ya tenés material para contestar. \
Respondé con lo que ya viste y ofrecé seguir si hace falta."
                .to_owned(),
            ok: false,
        };
    }

    let mut outcome = run(call);
    let taken = outcome.observation.chars().count();

    if taken > budget.left {
        let kept: String = outcome.observation.chars().take(budget.left).collect();
        outcome.observation = format!("{kept}\n\n[recortado: el turno ya leyó bastante]");
        budget.left = 0;
    } else {
        budget.left -= taken;
    }

    outcome
}

/// Runs one call. Every failure becomes text she can read and explain.
pub fn run(call: &ToolCall) -> ToolOutcome {
    let args = args_of(call);
    let summary = describe(call);

    let result = match call.name.as_str() {
        "read_file" => read_file(&args),
        "list_dir" => list_dir(&args),
        "search" => search(&args),
        "find" => find(&args),
        other => Err(format!(
            "no tengo una herramienta llamada «{other}». Tengo read_file, list_dir, search y find"
        )),
    };

    let (ok, observation) = match result {
        Ok(text) => (true, text),
        Err(err) => (false, format!("No pude: {err}")),
    };

    audit::append(&call.name, &args, &summary, ok, observation.len());

    ToolOutcome { summary, observation, ok }
}


fn read_file(args: &Value) -> Result<String, String> {
    let path = guard::resolve(string_arg(args, "path")?)?;
    let offset = number_arg(args, "offset").unwrap_or(0);
    let limit = number_arg(args, "limit").unwrap_or(DEFAULT_LINES).clamp(1, MAX_LINES);
    let numbered = args.get("numbers").and_then(Value::as_bool).unwrap_or(false);

    let text = extract::extract(&path)?;
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if total == 0 {
        return Ok(format!("{} está vacío.", path.display()));
    }

    let start = offset.min(total);
    if start == total {
        return Ok(format!(
            "{} tiene {total} líneas: ya lo leíste todo. Empezá de nuevo con offset=0 si querés releerlo.",
            path.display()
        ));
    }

    let end = (start + limit).min(total);
    let mut out = format!("# {} (líneas {}-{} de {})\n", path.display(), start + 1, end, total);

    if numbered {
        // `grep -n` prints `52:text`, so a citation she makes can be checked
        // against the same file without translating anything.
        for (index, line) in lines[start..end].iter().enumerate() {
            out.push_str(&format!("{}:{line}\n", start + index + 1));
        }
    } else {
        out.push_str(&lines[start..end].join("\n"));
    }

    if end < total {
        out.push_str(&format!(
            "\n\n[faltan {} líneas; seguí con offset={end}]",
            total - end
        ));
    }

    Ok(clip(out))
}

fn list_dir(args: &Value) -> Result<String, String> {
    let path = guard::resolve(string_arg(args, "path")?)?;
    if !path.is_dir() {
        return Err(format!("{} no es una carpeta. Para archivos usá read_file", path.display()));
    }

    let entries = std::fs::read_dir(&path).map_err(|err| format!("no puedo mirar ahí: {err}"))?;
    let mut rows: Vec<(char, String, u64)> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // `file_type` does not follow a link, which is the whole point: a link
        // listed as a file is a lie the next read pays for.
        let Ok(kind) = entry.file_type() else { continue };
        let mark = if kind.is_symlink() {
            'l'
        } else if kind.is_dir() {
            'd'
        } else {
            'f'
        };
        let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        rows.push((mark, name, size));
    }

    if rows.is_empty() {
        return Ok(format!("{} está vacía.", path.display()));
    }

    rows.sort_by(|left, right| {
        rank(left.0).cmp(&rank(right.0)).then_with(|| left.1.cmp(&right.1))
    });

    let shown = rows.len().min(MAX_ENTRIES);
    let mut out = format!("{} ({} entradas)\n", path.display(), rows.len());
    for (mark, name, size) in rows.iter().take(shown) {
        match mark {
            'd' => out.push_str(&format!("  d  {name}/\n")),
            'l' => out.push_str(&format!("  l  {name}\n")),
            _ => out.push_str(&format!("  f  {name}  ({} bytes)\n", size)),
        }
    }
    if rows.len() > shown {
        out.push_str(&format!("[…y {} más]\n", rows.len() - shown));
    }

    Ok(clip(out))
}

fn search(args: &Value) -> Result<String, String> {
    let query = string_arg(args, "query")?;
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Err("falta el texto a buscar".to_owned());
    }

    let root = guard::resolve(args.get("path").and_then(Value::as_str).unwrap_or("."))?;
    let limit = number_arg(args, "limit").unwrap_or(40).clamp(1, 100);
    let deep = args.get("deep").and_then(Value::as_bool).unwrap_or(false);
    let mut documents = MAX_DEEP_FILES;

    let mut hits: Vec<String> = Vec::new();
    let mut visited = 0usize;

    if root.is_file() {
        collect_hits(&root, &needle, limit, &mut hits);
    } else {
        let mut pending: Vec<(PathBuf, usize)> = vec![(root.clone(), 0)];

        while let Some((dir, depth)) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };

            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let child = entry.path();
                let Ok(meta) = entry.metadata() else { continue };

                if meta.is_dir() {
                    if depth < MAX_DEPTH && !SKIP_DIRS.contains(&name.as_str()) {
                        pending.push((child, depth + 1));
                    }
                    continue;
                }

                if meta.len() > MAX_FILE_BYTES {
                    continue;
                }
                // A document costs a parser per file. Deep is opt-in, and the
                // budget is what keeps "search the novel" from becoming "search
                // every document on the disk".
                if !extract::is_plain_text(&child) {
                    if !deep || documents == 0 {
                        continue;
                    }
                    documents -= 1;
                }
                if guard::deny(&child).is_err() {
                    continue;
                }

                visited += 1;
                collect_hits(&child, &needle, limit, &mut hits);

                if hits.len() >= limit || visited >= MAX_FILES {
                    break;
                }
            }

            if hits.len() >= limit || visited >= MAX_FILES {
                break;
            }
        }
    }

    if hits.is_empty() {
        let scope = if deep { ", adentro de los documentos también" } else { "" };
        return Ok(format!(
            "nada con «{}» en {} (miré {visited} archivos{scope})",
            query.trim(),
            root.display()
        ));
    }

    let mut out = format!(
        "{} coincidencias con «{}» en {}:\n",
        hits.len(),
        query.trim(),
        root.display()
    );
    out.push_str(&hits.join("\n"));
    Ok(clip(out))
}

/// Folders first, then links, then files.
fn rank(mark: char) -> u8 {
    match mark {
        'd' => 0,
        'l' => 1,
        _ => 2,
    }
}

fn collect_hits(file: &Path, needle: &str, limit: usize, hits: &mut Vec<String>) {
    let Ok(text) = extract::extract(file) else { return };

    for (number, line) in text.lines().enumerate() {
        if !line.to_lowercase().contains(needle) {
            continue;
        }
        hits.push(format!(
            "{}:{}: {}",
            file.display(),
            number + 1,
            cut(line.trim(), 200)
        ));
        if hits.len() >= limit {
            return;
        }
    }
}

/// By name, not by content: the question "where is that file" used to cost three
/// `list_dir` calls in a row.
fn find(args: &Value) -> Result<String, String> {
    let pattern = string_arg(args, "pattern")?.trim().to_owned();
    let needle = pattern.to_lowercase();
    if needle.is_empty() {
        return Err("falta el nombre a buscar".to_owned());
    }

    let root = guard::resolve(args.get("path").and_then(Value::as_str).unwrap_or("."))?;
    if !root.is_dir() {
        return Err(format!("{} no es una carpeta", root.display()));
    }
    let limit = number_arg(args, "limit").unwrap_or(MAX_FOUND).clamp(1, MAX_FOUND * 2);

    let mut found: Vec<String> = Vec::new();
    let mut pending: Vec<(PathBuf, usize)> = vec![(root.clone(), 0)];
    let mut visited = 0usize;

    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let child = entry.path();
            // `file_type` does not follow a link. That matters twice: walking a
            // linked directory can loop forever, and the guard already refuses
            // what the link points at.
            let Ok(kind) = entry.file_type() else { continue };
            let is_dir = kind.is_dir();

            if guard::deny(&child).is_err() {
                continue;
            }
            visited += 1;

            if name.to_lowercase().contains(&needle) {
                let line = if is_dir {
                    format!("{}/", child.display())
                } else if kind.is_symlink() {
                    format!("{}@", child.display())
                } else {
                    child.display().to_string()
                };
                found.push(line);
                if found.len() >= limit {
                    break;
                }
            }

            if is_dir && depth < MAX_DEPTH && !SKIP_DIRS.contains(&name.as_str()) {
                pending.push((child, depth + 1));
            }
        }

        if found.len() >= limit || visited >= MAX_FILES {
            break;
        }
    }

    if found.is_empty() {
        return Ok(format!(
            "ningún archivo con «{pattern}» en {} (miré {visited} entradas)",
            root.display()
        ));
    }

    let mut out = format!("{} con «{pattern}» en {}:\n", found.len(), root.display());
    out.push_str(&found.join("\n"));
    Ok(clip(out))
}

fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("falta «{key}»"))
}

fn number_arg(args: &Value, key: &str) -> Option<usize> {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}…")
}

fn short_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "la carpeta".to_owned();
    }

    let parts: Vec<&str> = trimmed.split('/').filter(|part| !part.is_empty()).collect();
    let mut kept: Vec<&str> = parts.iter().rev().take(2).rev().copied().collect();
    let mut out = kept.pop().unwrap_or_default().to_owned();
    if let Some(last) = kept.pop() {
        out = format!("{last}/{out}");
    }
    out
}

fn clip(text: String) -> String {
    if text.chars().count() <= MAX_OBSERVATION_CHARS {
        return text;
    }

    let kept: String = text.chars().take(MAX_OBSERVATION_CHARS).collect();
    format!("{kept}\n\n[recortado: pedime la parte siguiente]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nerea-tools-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall { id: "call-test".to_owned(), name: name.to_owned(), arguments: args.to_string() }
    }

    #[test]
    fn read_file_says_how_much_is_left() {
        let dir = scratch("window");
        let file = dir.join("long.txt");
        let body: String = (1..=10).map(|line| format!("linea {line}\n")).collect();
        fs::write(&file, body).expect("write");

        let outcome = run(&call(
            "read_file",
            serde_json::json!({ "path": file, "offset": 0, "limit": 4 }),
        ));

        assert!(outcome.ok, "{}", outcome.observation);
        assert!(outcome.observation.contains("linea 1"), "{}", outcome.observation);
        assert!(!outcome.observation.contains("linea 5"), "{}", outcome.observation);
        assert!(outcome.observation.contains("offset=4"), "{}", outcome.observation);
    }

    #[test]
    fn read_file_on_a_binary_explains_itself() {
        let dir = scratch("binary");
        let file = dir.join("blob.dat");
        fs::write(&file, [0, 1, 2, 3]).expect("write");

        let outcome = run(&call("read_file", serde_json::json!({ "path": file })));
        assert!(!outcome.ok);
        assert!(outcome.observation.contains("binario"), "{}", outcome.observation);
    }

    #[test]
    fn list_dir_puts_directories_first() {
        let dir = scratch("list");
        fs::create_dir(dir.join("src")).expect("dir");
        fs::write(dir.join("a.txt"), "hola").expect("write");

        let outcome = run(&call("list_dir", serde_json::json!({ "path": dir })));
        assert!(outcome.ok, "{}", outcome.observation);
        let src = outcome.observation.find("src/").expect("src listed");
        let file = outcome.observation.find("a.txt").expect("file listed");
        assert!(src < file, "{}", outcome.observation);
    }

    #[test]
    fn search_finds_the_line_and_its_file() {
        let dir = scratch("search");
        fs::write(dir.join("one.txt"), "nada\nKage es una sombra viva\n").expect("write");
        fs::write(dir.join("two.txt"), "otra cosa").expect("write");

        let outcome = run(&call("search", serde_json::json!({ "query": "kage", "path": dir })));
        assert!(outcome.ok, "{}", outcome.observation);
        assert!(outcome.observation.contains("one.txt:2:"), "{}", outcome.observation);
        assert!(outcome.observation.contains("Kage"), "{}", outcome.observation);
    }

    #[test]
    fn an_unknown_tool_is_a_message_not_a_panic() {
        let outcome = run(&call("rm_rf", serde_json::json!({})));
        assert!(!outcome.ok);
        assert!(outcome.observation.contains("read_file"), "{}", outcome.observation);
    }

    #[test]
    fn the_describe_line_is_short_and_readable() {
        let described = describe(&call(
            "read_file",
            serde_json::json!({ "path": "/home/ramses/Proyectos/nerea/src-tauri/src/lib.rs" }),
        ));
        assert_eq!(described, "leyendo src/lib.rs");
    }

    #[test]
    fn the_observation_never_grows_past_its_budget() {
        let clipped = clip("x".repeat(MAX_OBSERVATION_CHARS + 500));
        assert!(clipped.chars().count() < MAX_OBSERVATION_CHARS + 100);
        assert!(clipped.contains("recortado"));
    }

    #[test]
    fn the_turn_budget_refuses_the_third_full_observation() {
        let dir = scratch("budget");
        let file = dir.join("big.txt");
        let body: String = (0..400).map(|index| format!("{index:04} {}\n", "x".repeat(40))).collect();
        fs::write(&file, body).expect("big file");

        let mut budget = Budget::per_message();
        for _ in 0..2 {
            let outcome = run_charged(
                &call("read_file", serde_json::json!({ "path": file, "limit": MAX_LINES })),
                &mut budget,
            );
            assert!(outcome.ok, "{}", outcome.observation);
            assert!(outcome.observation.chars().count() > 7_000, "una lectura llena");
        }

        let refused = run_charged(&call("read_file", serde_json::json!({ "path": file })), &mut budget);
        assert!(!refused.ok, "el tercer turno ya no lee");
        assert!(refused.observation.contains("Respondé"), "{}", refused.observation);
        assert_eq!(budget.left(), 0);
    }

    #[test]
    fn a_call_that_overflows_the_budget_keeps_what_fits() {
        let dir = scratch("budget-partial");
        let file = dir.join("big.txt");
        let body: String = (0..400).map(|index| format!("{index:04} {}\n", "x".repeat(40))).collect();
        fs::write(&file, body).expect("big file");

        let mut budget = Budget::per_message();
        run_charged(&call("read_file", serde_json::json!({ "path": file })), &mut budget);
        let partial = run_charged(&call("read_file", serde_json::json!({ "path": file })), &mut budget);

        assert!(partial.ok, "{}", partial.observation);
        assert!(partial.observation.contains("recortado"), "{}", partial.observation);
        assert_eq!(budget.left(), 0);
    }

    /// `grep -n` prints `52:text`, so a citation she makes has to match it.
    #[test]
    fn numbers_prefix_each_line_when_asked() {
        let dir = scratch("numbers");
        let file = dir.join("cap.txt");
        fs::write(&file, "primera\nsegunda\ntercera\n").expect("write");

        let plain = run(&call("read_file", serde_json::json!({ "path": file })));
        assert!(plain.ok, "{}", plain.observation);
        assert!(!plain.observation.contains("2:segunda"), "{}", plain.observation);

        let numbered = run(&call("read_file", serde_json::json!({ "path": file, "numbers": true })));
        assert!(numbered.ok, "{}", numbered.observation);
        assert!(numbered.observation.contains("2:segunda"), "{}", numbered.observation);
        assert!(numbered.observation.contains("3:tercera"), "{}", numbered.observation);
    }

    /// A listing that calls a link a file is a listing that lies.
    #[test]
    fn list_dir_names_the_kind_of_each_entry() {
        let dir = scratch("kinds");
        fs::create_dir(dir.join("Novela")).expect("dir");
        fs::write(dir.join("notas.txt"), "hola").expect("write");
        std::os::unix::fs::symlink(dir.join("notas.txt"), dir.join("atajo")).expect("link");

        let outcome = run(&call("list_dir", serde_json::json!({ "path": dir })));

        assert!(outcome.ok, "{}", outcome.observation);
        assert!(outcome.observation.contains("d  Novela/"), "{}", outcome.observation);
        assert!(outcome.observation.contains("f  notas.txt"), "{}", outcome.observation);
        assert!(outcome.observation.contains("l  atajo"), "{}", outcome.observation);
    }

    /// A picture is not a mystery: saying what it is beats a failed parse.
    #[test]
    fn a_picture_says_it_cannot_be_read() {
        let dir = scratch("picture");
        let file = dir.join("foto.png");
        fs::write(&file, [0x89, b'P', b'N', b'G']).expect("write");

        let outcome = run(&call("read_file", serde_json::json!({ "path": file })));

        assert!(!outcome.ok);
        assert!(outcome.observation.contains("imágenes"), "{}", outcome.observation);
        assert!(!outcome.observation.contains("binario"), "{}", outcome.observation);
    }

    /// A minimal `.docx`: Word keeps the text in `word/document.xml` and nowhere
    /// else, so a zip with that one part is a real document for the reader.
    fn write_docx(path: &Path, text: &str) {
        use std::io::Write;

        let file = fs::File::create(path).expect("create docx");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();

        zip.start_file("word/document.xml", options).expect("document part");
        let body = format!(
            r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#
        );
        zip.write_all(body.as_bytes()).expect("write document");
        zip.finish().expect("finish docx");
    }

    /// The novel is a `.docx`: a search that cannot read inside it only finds it
    /// by accident. Deep is opt-in because unpacking every document is expensive.
    #[test]
    fn search_turns_deep_only_when_asked() {
        let dir = scratch("deep");
        write_docx(&dir.join("novela.docx"), "Nerea vive en el faro");
        fs::write(dir.join("notas.txt"), "Nerea camina por la costa").expect("plain note");

        let shallow = run(&call("search", serde_json::json!({ "query": "Nerea", "path": dir })));
        assert!(shallow.ok, "{}", shallow.observation);
        assert!(shallow.observation.contains("notas.txt"), "{}", shallow.observation);
        assert!(!shallow.observation.contains("novela.docx"), "{}", shallow.observation);

        let deep = run(&call(
            "search",
            serde_json::json!({ "query": "Nerea", "path": dir, "deep": true }),
        ));
        assert!(deep.ok, "{}", deep.observation);
        assert!(deep.observation.contains("novela.docx"), "{}", deep.observation);
        assert!(deep.observation.contains("notas.txt"), "{}", deep.observation);
    }

    /// One damaged document is skipped, not fatal: a bad zip cannot take the
    /// whole search down with it.
    #[test]
    fn deep_search_skips_a_broken_document() {
        let dir = scratch("broken");
        fs::write(dir.join("roto.docx"), b"esto no es un zip").expect("broken docx");
        fs::write(dir.join("notas.txt"), "Nerea camina").expect("plain note");

        let outcome = run(&call(
            "search",
            serde_json::json!({ "query": "Nerea", "path": dir, "deep": true }),
        ));

        assert!(outcome.ok, "{}", outcome.observation);
        assert!(outcome.observation.contains("notas.txt"), "{}", outcome.observation);
    }

    /// She used `list_dir` three times in a row to find one file; a name is a
    /// better question than a folder.
    #[test]
    fn find_matches_by_name_and_marks_folders() {
        let dir = scratch("find");
        fs::create_dir_all(dir.join("Novela")).expect("folder");
        fs::write(dir.join("Novela").join("Nerea.docx"), b"x").expect("nested file");
        fs::write(dir.join("otra-cosa.txt"), b"x").expect("other file");

        let by_file = run(&call("find", serde_json::json!({ "pattern": "NEREA", "path": dir })));
        assert!(by_file.ok, "{}", by_file.observation);
        assert!(by_file.observation.contains("Nerea.docx"), "{}", by_file.observation);
        assert!(!by_file.observation.contains("otra-cosa.txt"), "{}", by_file.observation);

        let by_folder = run(&call("find", serde_json::json!({ "pattern": "novela", "path": dir })));
        assert!(by_folder.ok, "{}", by_folder.observation);
        assert!(by_folder.observation.contains("Novela/"), "{}", by_folder.observation);
    }

    #[test]
    fn find_stops_at_the_limit_it_was_given() {
        let dir = scratch("find-limit");
        for index in 0..5 {
            fs::write(dir.join(format!("hit-{index}.txt")), b"x").expect("file");
        }

        let outcome = run(&call(
            "find",
            serde_json::json!({ "pattern": "hit-", "path": dir, "limit": 2 }),
        ));

        assert!(outcome.ok, "{}", outcome.observation);
        let listed = outcome.observation.lines().filter(|line| line.ends_with(".txt")).count();
        assert_eq!(listed, 2, "{}", outcome.observation);
    }
}
