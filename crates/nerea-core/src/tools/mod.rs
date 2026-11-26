//! What she may do with the machine: look, and keep a note of her own.
//!
//! Four tools read; the fifth, `remember`, writes — and only inside her own
//! cards directory, never a path the model hands in.
//!
//! She never gets a shell. Every capability is a named function with a typed
//! argument shape, declared to the model as a tool. The API says when she wants
//! one — that is why there is no JSON protocol in the prompt: a model that says
//! "voy a buscar" and stops cannot happen, because the call is a field, not a
//! sentence. Every call lands in the audit log.

pub mod audit;
pub mod extract;
pub mod guard;
pub mod system;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ai::{ToolCall, ToolSpec};
use crate::cards::{self, Kept};
use crate::changes;
use crate::apps::{self, Lookup};

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
        ToolSpec {
            name: "docs".to_owned(),
            description: "Look up current, versioned documentation for a programming library \
(context7): real API signatures instead of what the model remembers. Use it before writing code \
against a library, and never guess a signature. `library` is a package name like \"tokio\" or a \
path like \"/tokio-rs/tokio\"; `question` is what you need to know, in one sentence."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "library": { "type": "string", "description": "Package name or /org/project" },
                    "question": { "type": "string", "description": "What you need to know, one sentence" }
                },
                "required": ["library", "question"]
            }),
        },
        ToolSpec {
            name: "run_command".to_owned(),
            description: "Run a program on his machine, after he approves it. There is no \
shell: `program` is a name from his allowlist and `args` is that program's own argv, so `;`, \
`|` and `$(…)` are not possible. Every command opens a card with the exact argv and waits for \
him. Deleting is only allowed inside his declared folders; `sudo` never runs."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "program": {
                        "type": "string",
                        "description": "The program name, like pandoc or playerctl — never a path"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Its arguments, one per element, exactly as the program expects"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Folder to run in. Defaults to his home"
                    },
                    "reason": {
                        "type": "string",
                        "description": "One line for him: why this command"
                    }
                },
                "required": ["program", "args"]
            }),
        },
        ToolSpec {
            name: "open_app".to_owned(),
            description: "Open an application installed on his desktop: Spotify, Brave, a game. \
Pass the name the way he said it. The program and its arguments come from that app's .desktop \
file — never from you — so this can only open something already installed. If two apps match, \
the answer lists them and you ask which one. With no `name` it returns the full list."
                .to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Application name, like spotify, brave or unity"
                    }
                },
                "required": []
            }),
        },
        ToolSpec {
            name: "write_file".to_owned(),
            description: "Propose writing a whole file: new, or replacing an existing one. It \
does NOT write by itself — the change waits for Lioner to approve it in the chat, and he sees \
the diff first. Use it when he asks you to create or rewrite a file."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path, or one starting with ~"
                    },
                    "content": {
                        "type": "string",
                        "description": "The complete new content of the file"
                    },
                    "reason": {
                        "type": "string",
                        "description": "One line for him: why this change"
                    }
                },
                "required": ["path", "content"]
            }),
        },
        ToolSpec {
            name: "edit_file".to_owned(),
            description: "Propose replacing one exact fragment of an existing file. The fragment \
must appear exactly once. It does NOT write by itself: the diff waits for Lioner's approval in \
the chat."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path, or one starting with ~" },
                    "find": {
                        "type": "string",
                        "description": "The exact text to replace, unique in the file"
                    },
                    "replace": { "type": "string", "description": "What takes its place" },
                    "reason": { "type": "string", "description": "One line for him: why" }
                },
                "required": ["path", "find", "replace"]
            }),
        },
        ToolSpec {
            name: "processes".to_owned(),
            description: "List the processes running on the machine, with CPU and memory. \
Read-only: it never starts, stops or signals anything. Sorted by memory by default; use \
`sort: \"cpu\"` to find what is hot, or `limit` to see more than the first 30."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "description": "How many processes to show. Default 30, max 5000"
                    },
                    "sort": {
                        "type": "string",
                        "description": "\"mem\" (default), \"cpu\" or \"pid\""
                    }
                }
            }),
        },
        ToolSpec {
            name: "system".to_owned(),
            description: "The machine's health in one answer: memory and swap in use, load \
average, how long it has been up, and how full each disk is. Read-only."
                .to_owned(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "services".to_owned(),
            description: "Which systemd units have failed. Read-only: it reports what is \
broken, it does not restart anything."
                .to_owned(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "logs".to_owned(),
            description: "Recent entries from the system journal, errors by default. Read-only."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "lines": {
                        "type": "integer",
                        "description": "How many entries. Default 20, max 80"
                    },
                    "priority": {
                        "type": "string",
                        "description": "\"err\" (default), \"warn\", \"crit\" or \"info\""
                    }
                }
            }),
        },
        ToolSpec {
            name: "remember".to_owned(),
            description: "Save a durable fact in your own memory cards, one card per topic. Use it \
ONLY when the user explicitly asks you to remember something important (\"recuerda esto\", \"anotá \
que…\", \"no te olvides de…\"). Never on your own initiative, and never for greetings, small talk \
or a passing preference: those belong to the conversation, not to a card. It never deletes a fact, \
and one card holds at most 2 KB — if the fact does not fit, say so instead of dropping something."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "One word naming the card, like lore_novela or proyectos_actuales"
                    },
                    "fact": {
                        "type": "string",
                        "description": "The fact to keep, as one sentence"
                    },
                    "title": {
                        "type": "string",
                        "description": "A readable title. Only used when the card is new"
                    },
                    "tags": {
                        "type": "string",
                        "description": "Comma-separated words the user would say when this matters, like \"novela, capitulos\""
                    }
                },
                "required": ["topic", "fact"]
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
        "remember" => format!("anotando en «{}»", cut(text("topic"), 30)),
        "docs" => format!("buscando documentación de «{}»", cut(text("library"), 30)),
        "run_command" => {
            let args = args.get("args").and_then(Value::as_array).cloned().unwrap_or_default();
            let rendered: Vec<String> =
                args.iter().map(|arg| arg.as_str().unwrap_or_default().to_owned()).collect();
            format!("ejecutando {} {}", cut(text("program"), 20), cut(&rendered.join(" "), 50))
        }
        "open_app" => format!("abriendo «{}»", cut(text("name"), 30)),
        "write_file" => format!("proponiendo escribir {}", short_path(text("path"))),
        "edit_file" => format!("proponiendo un cambio en {}", short_path(text("path"))),
        "processes" => "mirando los procesos".to_owned(),
        "system" => "mirando la salud de la máquina".to_owned(),
        "services" => "mirando los servicios".to_owned(),
        "logs" => "leyendo los errores del sistema".to_owned(),
        other => format!("usando {other}"),
    }
}

/// Why a turn ended without words. A blank answer used to be a shrug; these
/// are the facts that turn it into a sentence he can act on.
#[derive(Debug, Default, Clone)]
pub struct BlankFacts {
    /// Tool calls that actually ran this turn.
    pub calls: usize,
    /// One line per tool that failed, with its own reason.
    pub failed: Vec<String>,
    /// The turn used its whole reading budget.
    pub budget_exhausted: bool,
    /// The final round still asked for a tool it could not have.
    pub asked_too_late: bool,
    /// Rounds that ended without a single word from the model.
    pub empty_rounds: usize,
}

/// The line she says when she has nothing to say, explaining why.
pub fn blank_reason(facts: &BlankFacts) -> String {
    let (reason, hint) = if facts.asked_too_late {
        (
            "pedí una herramienta en la última ronda y ya no quedaba turno",
            "Pedímelo otra vez y lo hago primero, sin hablar antes.",
        )
    } else if facts.budget_exhausted {
        (
            "se me acabó el presupuesto de lectura del turno",
            "Partí el pedido en dos: primero lo que querés mirar, después lo que querés hacer.",
        )
    } else if !facts.failed.is_empty() {
        (
            "las herramientas fallaron antes de poder contestarte",
            "El detalle está en la línea de abajo; decime si sigo por ahí.",
        )
    } else if facts.calls == 0 {
        (
            "el proveedor no devolvió texto",
            "Probá de nuevo; si se repite, el problema es del servicio, no tuyo.",
        )
    } else {
        ("no llegó texto en ninguna de las rondas", "Probá reformular el pedido, más corto.")
    };

    let mut text = format!("No te contesté: {reason}.\n\n{hint}");

    if !facts.failed.is_empty() {
        let detail: Vec<String> = facts.failed.iter().take(3).cloned().collect();
        text.push_str(&format!("\n\n{}", detail.join("\n")));
    }
    if facts.empty_rounds > 1 {
        text.push_str(&format!("\n\n({} rondas sin texto)", facts.empty_rounds));
    }

    text
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

/// Charges an observation that did not come from `run_charged`, like a command
/// the shell had to run asynchronously.
pub fn charge(mut outcome: ToolOutcome, budget: &mut Budget) -> ToolOutcome {
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
        "open_app" => open_app(&args),
        "write_file" => propose_write(&args),
        "edit_file" => propose_edit(&args),
        "processes" => {
            let limit = number_arg(&args, "limit").unwrap_or(30);
            let sort = system::Sort::parse(args.get("sort").and_then(Value::as_str).unwrap_or("mem"));
            system::processes(limit, sort)
        }
        "system" => system::vitals(),
        "services" => system::services(),
        "logs" => system::logs(
            number_arg(&args, "lines").unwrap_or(20),
            args.get("priority").and_then(Value::as_str).unwrap_or("err"),
        ),
        "remember" => keep(&args, &cards::dir()),
        other => Err(format!(
            "no tengo una herramienta llamada «{other}». Tengo read_file, list_dir, search, find, \
remember, processes, system, services, logs, open_app, write_file, edit_file, run_command y \
docs"
        )),
    };

    let (ok, observation) = match result {
        Ok(text) => (true, text),
        Err(err) => (false, format!("No pude: {err}")),
    };

    audit::append(&call.name, &args, &summary, ok, observation.len());

    ToolOutcome { summary, observation, ok }
}


/// Opens one installed application. The name is all the model chooses: the
/// program and every argument come from the `.desktop` file, so there is no
/// room for `spotify --flag-you-invented`.
fn open_app(args: &Value) -> Result<String, String> {
    let query = args.get("name").and_then(Value::as_str).unwrap_or_default().trim();
    let installed = apps::catalog();

    if installed.is_empty() {
        return Err(
            "no encontré ninguna aplicación instalada: miré los .desktop del sistema y no hay \
ninguno que pueda lanzar"
                .to_owned(),
        );
    }

    if query.is_empty() {
        return Ok(format!(
            "Estas son las aplicaciones instaladas ({}): {}",
            installed.len(),
            names(&installed, 60)
        ));
    }

    match apps::resolve(&installed, query) {
        Lookup::One(app) => {
            apps::launch(&app)?;
            Ok(format!("Abrí «{}».", app.name))
        }
        Lookup::Many(found) => Ok(format!(
            "Hay varias parecidas a «{}»: {}. Decime cuál y la abro.",
            cut(query, 30),
            found.iter().take(6).map(|app| app.label()).collect::<Vec<_>>().join(", ")
        )),
        Lookup::None => Ok(format!(
            "No tengo «{}» instalada. Entre las que hay: {}",
            cut(query, 30),
            names(&installed, 40)
        )),
    }
}

fn names(installed: &[apps::App], limit: usize) -> String {
    let list: Vec<&str> = installed.iter().map(|app| app.name.as_str()).take(limit).collect();
    let extra = installed.len().saturating_sub(list.len());
    let mut text = list.join(", ");
    if extra > 0 {
        text.push_str(&format!(" y {extra} más"));
    }
    text
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

/// Stages a whole-file write. Nothing touches the target until he approves.
fn propose_write(args: &Value) -> Result<String, String> {
    let path = string_arg(args, "path")?;
    let content = string_arg(args, "content")?;
    let change = changes::stage(path, content)?;

    Ok(format!(
        "Propuesta {} lista (todavía no escribí nada): {}. Espera que Lioner la apruebe en el chat.",
        change.id, change.path
    ))
}

fn propose_edit(args: &Value) -> Result<String, String> {
    let path = string_arg(args, "path")?;
    let find = string_arg(args, "find")?;
    let replace = string_arg(args, "replace")?;
    let change = changes::edit(path, find, replace)?;

    Ok(format!(
        "Propuesta {} lista (todavía no escribí nada): cambio en {}. Espera que Lioner lo apruebe \
en el chat.",
        change.id, change.path
    ))
}

/// The only writing tool, and it takes no path: the file name comes from a
/// slug, so a `topic` can never point outside the cards directory.
fn keep(args: &Value, dir: &Path) -> Result<String, String> {
    let topic = string_arg(args, "topic")?;
    let fact = string_arg(args, "fact")?;
    let title = args.get("title").and_then(Value::as_str);
    let tags = args.get("tags").and_then(Value::as_str).unwrap_or_default();

    let kept = cards::remember(dir, topic, title, tags, fact)?;
    let name = |path: &Path| {
        path.file_stem().and_then(|stem| stem.to_str()).unwrap_or(topic).to_owned()
    };

    Ok(match kept {
        Kept::Created { path, facts } => format!(
            "Anotado en la tarjeta nueva «{}», que ahora tiene {facts} hechos. Vive en {}.",
            name(&path),
            path.display()
        ),
        Kept::Updated { path, facts } => format!(
            "Anotado en la tarjeta «{}», que ahora tiene {facts} hechos.",
            name(&path)
        ),
        Kept::AlreadyThere { .. } => "Eso ya estaba anotado: no lo dupliqué.".to_owned(),
    })
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
        for name in ["read_file", "list_dir", "search", "find", "remember"] {
            assert!(outcome.observation.contains(name), "{}", outcome.observation);
        }
    }

    /// The tool layer is the seam: `keep` takes the directory it writes to, so
    /// this test never depends on `NEREA_DATA_DIR` or on a real card.
    #[test]
    fn remember_says_what_it_did_and_does_not_duplicate() {
        let dir = scratch("tool-remember");
        let args = serde_json::json!({
            "topic": "lore_novela",
            "title": "La novela",
            "tags": "novela, capitulos",
            "fact": "Nerea vive en el faro"
        });

        let first = keep(&args, &dir).expect("written");
        assert!(first.contains("tarjeta nueva"), "{first}");
        assert!(first.contains("1 hechos"), "{first}");
        assert!(dir.join("lore_novela.md").is_file());

        let again = keep(&args, &dir).expect("still fine");
        assert!(again.contains("ya estaba"), "{again}");
    }

    #[test]
    fn remember_refuses_a_topic_that_is_a_path() {
        let dir = scratch("tool-remember-topic");
        let args = serde_json::json!({ "topic": "../fuera", "fact": "un hecho" });

        let err = keep(&args, &dir).expect_err("must refuse");
        assert!(err.contains("ruta"), "{err}");
        assert!(!dir.join("..").join("fuera.md").exists());
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

    #[test]
    fn a_blank_turn_names_the_budget_when_the_budget_ran_out() {
        let facts = BlankFacts { calls: 3, budget_exhausted: true, ..BlankFacts::default() };
        let line = blank_reason(&facts);

        assert!(line.contains("presupuesto"), "{line}");
        assert!(line.contains("Partí el pedido"), "{line}");
    }

    #[test]
    fn a_blank_turn_shows_which_tool_failed() {
        let facts = BlankFacts {
            calls: 1,
            failed: vec!["ejecutando playerctl: «playerctl» no está en tu lista".to_owned()],
            ..BlankFacts::default()
        };
        let line = blank_reason(&facts);

        assert!(line.contains("fallaron"), "{line}");
        assert!(line.contains("no está en tu lista"), "el detalle tiene que verse: {line}");
    }

    #[test]
    fn a_blank_turn_without_tools_blames_the_provider() {
        let facts = BlankFacts { empty_rounds: 1, ..BlankFacts::default() };
        let line = blank_reason(&facts);

        assert!(line.contains("el proveedor no devolvió texto"), "{line}");
        assert!(line.contains("el problema es del servicio"), "{line}");
    }

    #[test]
    fn a_blank_turn_says_when_the_last_round_asked_too_late() {
        let facts = BlankFacts {
            calls: 2,
            asked_too_late: true,
            empty_rounds: 1,
            ..BlankFacts::default()
        };
        let line = blank_reason(&facts);

        assert!(line.contains("última ronda"), "{line}");
        assert!(line.contains("lo hago primero"), "{line}");
    }
}
