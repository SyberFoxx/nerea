//! Read-only access to the machine: what she may look at, and how.
//!
//! She never gets a shell. Every capability is a named function with a typed
//! argument shape; the model asks for one with a fenced block, the app runs it
//! and hands back the text. That keeps the provider contract untouched — no
//! dependence on native function calling — and every call lands in the audit
//! log.

pub mod audit;
pub mod extract;
pub mod guard;

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Each call is another round trip; three is enough to find something and read it.
pub const MAX_CALLS_PER_MESSAGE: usize = 3;

const FENCE: &str = "```nerea-tool";
const DEFAULT_LINES: usize = 300;
const MAX_LINES: usize = 1_200;
/// Roughly 3k tokens: a fifth of the context budget, never more.
const MAX_OBSERVATION_CHARS: usize = 12_000;
const MAX_ENTRIES: usize = 200;
const MAX_FILES: usize = 4_000;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_DEPTH: usize = 6;
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

/// The block she is told to answer with when she needs to look something up.
pub const PROTOCOL: &str = r#"

## Leer la máquina
Tenés herramientas de solo lectura sobre esta máquina. Cuando necesites una, respondé ÚNICAMENTE con un bloque así, sin nada más:

```nerea-tool
{"tool": "read_file", "args": {"path": "/ruta/al/archivo"}}
```

- `read_file {"path", "offset"?, "limit"?}` — texto, código, Markdown, JSON, PDF, Word (.docx), OpenDocument (.odt), EPUB y HTML. `offset` es la línea donde empezás (0 = principio) y `limit` cuántas líneas; sin `limit` devuelve 300.
- `list_dir {"path"}` — qué hay en una carpeta.
- `search {"query", "path", "limit"?}` — busca ese texto dentro de los archivos de una carpeta.

Podés escribir una frase antes del bloque, para contarle qué vas a hacer, pero el bloque cierra tu mensaje: nunca escribas nada después. Si querés decir algo más, esperá el resultado.

Reglas: no inventes rutas; si no sabés dónde está algo, usá `list_dir` o `search`. Aceptás `~` y rutas relativas. Cuando llega el resultado, contestás a Lioner con tu voz normal, sin el bloque, sin repetir el texto entero. Si lo que necesitás es más largo, pedí la parte siguiente con `offset`. Máximo 3 herramientas por mensaje."#;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub args: Value,
    /// The block exactly as she wrote it, echoed back so the thread stays coherent.
    pub raw: String,
}

#[derive(Debug, Clone)]
pub struct ToolOutcome {
    /// Short label for the UI, in her voice.
    pub summary: String,
    /// What the model reads.
    pub observation: String,
    pub ok: bool,
}

/// Finds a tool block anywhere in a reply. `None` means it is ordinary prose.
pub fn parse(reply: &str) -> Option<ToolCall> {
    let start = reply.find(FENCE)?;
    let body_start = start + FENCE.len();
    let end = reply[body_start..].find("```")? + body_start;
    let raw = reply[start..end + 3].to_owned();

    let parsed: Value = serde_json::from_str(reply[body_start..end].trim()).ok()?;
    let name = parsed.get("tool")?.as_str()?.to_owned();
    let args = parsed.get("args").cloned().unwrap_or(Value::Null);

    Some(ToolCall { name, args, raw })
}

/// One line for her subtitle while the tool runs.
pub fn describe(call: &ToolCall) -> String {
    let text = |key: &str| call.args.get(key).and_then(Value::as_str).unwrap_or_default();
    match call.name.as_str() {
        "read_file" => format!("leyendo {}", short_path(text("path"))),
        "list_dir" => format!("mirando {}", short_path(text("path"))),
        "search" => format!("buscando «{}»", cut(text("query"), 40)),
        other => format!("usando {other}"),
    }
}

/// Runs one call. Every failure becomes text she can read and explain.
pub fn run(call: &ToolCall) -> ToolOutcome {
    let summary = describe(call);

    let result = match call.name.as_str() {
        "read_file" => read_file(&call.args),
        "list_dir" => list_dir(&call.args),
        "search" => search(&call.args),
        other => Err(format!(
            "no tengo una herramienta llamada «{other}». Tengo read_file, list_dir y search"
        )),
    };

    let (ok, observation) = match result {
        Ok(text) => (true, text),
        Err(err) => (false, format!("No pude: {err}")),
    };

    audit::append(&call.name, &call.args, &summary, ok, observation.len());

    ToolOutcome { summary, observation, ok }
}

fn read_file(args: &Value) -> Result<String, String> {
    let path = guard::resolve(string_arg(args, "path")?)?;
    let offset = number_arg(args, "offset").unwrap_or(0);
    let limit = number_arg(args, "limit").unwrap_or(DEFAULT_LINES).clamp(1, MAX_LINES);

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
    out.push_str(&lines[start..end].join("\n"));

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
    let mut rows: Vec<(bool, String, u64)> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let (is_dir, size) = match entry.metadata() {
            Ok(meta) => (meta.is_dir(), meta.len()),
            Err(_) => (false, 0),
        };
        rows.push((is_dir, name, size));
    }

    if rows.is_empty() {
        return Ok(format!("{} está vacía.", path.display()));
    }

    rows.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));

    let shown = rows.len().min(MAX_ENTRIES);
    let mut out = format!("{} ({} entradas)\n", path.display(), rows.len());
    for (is_dir, name, size) in rows.iter().take(shown) {
        if *is_dir {
            out.push_str(&format!("  {name}/\n"));
        } else {
            out.push_str(&format!("  {name}  ({} bytes)\n", size));
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

                if meta.len() > MAX_FILE_BYTES || !extract::is_plain_text(&child) {
                    continue;
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
        return Ok(format!(
            "nada con «{}» en {} (miré {visited} archivos)",
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
        ToolCall { name: name.to_owned(), args, raw: String::new() }
    }

    #[test]
    fn a_tool_block_is_recognized() {
        let reply = "```nerea-tool\n{\"tool\": \"read_file\", \"args\": {\"path\": \"/tmp/x\"}}\n```";
        let call = parse(reply).expect("call");
        assert_eq!(call.name, "read_file");
        assert_eq!(call.args["path"], "/tmp/x");
    }

    #[test]
    fn an_ordinary_answer_is_not_a_tool_call() {
        assert!(parse("Hola, mi dragón. ¿En qué andás?").is_none());
    }

    #[test]
    fn a_code_fence_that_is_not_a_tool_is_ignored() {
        assert!(parse("Mirá:\n```rust\nfn main() {}\n```").is_none());
    }

    #[test]
    fn broken_json_is_not_a_crash() {
        assert!(parse("```nerea-tool\n{ not json }\n```").is_none());
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
        let file = dir.join("sound.wav");
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
}
