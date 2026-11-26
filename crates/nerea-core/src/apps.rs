//! Her hands on the desktop: opening an application that is already installed.
//!
//! The model picks a name. The program and its arguments come from the
//! `.desktop` file Lioner has installed, never from the model, so "abrí spotify"
//! cannot quietly become "abrí spotify --lo-que-sea": the whole argv is written
//! by the distribution, and the only thing the model controls is which entry.
//!
//! Flatpak entries are rebuilt as `flatpak run <id>` instead of being copied:
//! their `Exec` carries `--file-forwarding … @@u %U @@`, and handing those
//! markers to `flatpak` without files is how a launcher fails for no reason.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Programs that are not applications: a `.desktop` file asking for a shell is
/// a script in disguise, and this tool does not run scripts.
const NOT_AN_APP: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "csh", "tcsh", "sudo", "pkexec", "doas", "su",
    "systemd-run", "python", "python3", "perl", "ruby", "node", "osascript",
];

/// One launchable entry, already resolved to an argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// The name a person says: `Name=` from the file.
    pub name: String,
    /// The file stem: `com.spotify.Client`.
    pub id: String,
    /// Exactly what gets spawned.
    pub exec: Vec<String>,
}

impl App {
    /// What she shows when the name is ambiguous.
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.id)
    }
}

/// What a name search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    One(App),
    Many(Vec<App>),
    None,
}

/// Every application on this machine, alphabetically, without duplicates.
pub fn catalog() -> Vec<App> {
    let mut seen: Vec<App> = Vec::new();

    for dir in data_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };

        let mut files: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "desktop"))
            .collect();
        files.sort();

        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let id = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let Some(app) = parse_entry(&text, &id) else {
                continue;
            };
            if !program_exists(&app.exec[0]) {
                continue;
            }
            // A user file shadows the system one with the same name.
            let dup = seen.iter().any(|other| {
                other.name.eq_ignore_ascii_case(&app.name) || other.id == app.id
            });
            if !dup {
                seen.push(app);
            }
        }
    }

    seen.sort_by_key(|app| app.name.to_lowercase());
    seen
}

/// Resolves a spoken name to exactly one application.
pub fn resolve(apps: &[App], query: &str) -> Lookup {
    let wanted = normalize(query);
    if wanted.is_empty() {
        return Lookup::None;
    }

    let mut scored: Vec<(u8, &App)> = apps
        .iter()
        .filter_map(|app| score(app, &wanted).map(|points| (points, app)))
        .collect();

    if scored.is_empty() {
        return Lookup::None;
    }

    scored.sort_by_key(|(points, _)| std::cmp::Reverse(*points));
    let best = scored[0].0;

    let winners: Vec<App> =
        scored.iter().filter(|(points, _)| *points == best).map(|(_, app)| (*app).clone()).collect();

    let exact = best >= 100;
    if winners.len() == 1 || exact {
        Lookup::One(winners[0].clone())
    } else {
        Lookup::Many(winners)
    }
}

/// Spawns one entry and reaps it in the background, so the child does not stay
/// as a zombie if it exits on its own.
pub fn launch(app: &App) -> Result<(), String> {
    if app.exec.is_empty() {
        return Err(format!("«{}» no tiene un programa que ejecutar", app.name));
    }

    let mut command = Command::new(&app.exec[0]);
    command
        .args(&app.exec[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = command.spawn().map_err(|err| {
        format!("no pude lanzar «{}» ({}): {err}", app.name, app.exec[0])
    })?;

    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });

    Ok(())
}

/// `input` normalized for comparing what a person says with what a file says.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for ch in text.chars().flat_map(|ch| ch.to_lowercase()) {
        let folded = match ch {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        };
        if folded.is_alphanumeric() {
            out.push(folded);
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.trim().to_owned()
}

/// 100 for the name itself, 80 for a prefix, 60 for a word inside the name,
/// 50 for the id. Two different apps can tie, and then the model gets a list.
fn score(app: &App, wanted: &str) -> Option<u8> {
    let name = normalize(&app.name);
    let id = normalize(&app.id);

    if name == wanted || id == wanted {
        return Some(100);
    }
    if name.split(' ').any(|word| word == wanted) {
        return Some(90);
    }
    if name.starts_with(wanted) {
        return Some(80);
    }
    if id.starts_with(wanted) {
        return Some(75);
    }
    if name.contains(wanted) {
        return Some(60);
    }
    if id.contains(wanted) {
        return Some(50);
    }
    None
}

fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs = Vec::new();

    let user_data = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("{home}/.local/share"));
    dirs.push(PathBuf::from(user_data).join("applications"));

    for extra in ["/usr/local/share", "/usr/share"] {
        dirs.push(PathBuf::from(extra).join("applications"));
    }

    // Flatpak exports: the `.desktop` files that actually launch the sandbox.
    if !home.is_empty() {
        dirs.push(PathBuf::from(&home).join(".local/share/flatpak/exports/share/applications"));
    }
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));

    dirs
}

/// Reads the `[Desktop Entry]` section of one file. `None` when it is not an
/// application a person would ask for.
fn parse_entry(text: &str, id: &str) -> Option<App> {
    let mut section = String::new();
    let mut name = String::new();
    let mut exec = String::new();
    let mut kind = String::new();
    let mut hidden = false;

    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']).to_owned();
            continue;
        }
        if section != "Desktop Entry" {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Name" => {
                if name.is_empty() {
                    name = value.to_owned();
                }
            }
            "Exec" => exec = value.to_owned(),
            "Type" => kind = value.to_owned(),
            "Hidden" | "NoDisplay" => hidden |= value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }

    if hidden || name.is_empty() || exec.is_empty() {
        return None;
    }
    if !kind.is_empty() && kind != "Application" {
        return None;
    }

    let terminal = text.lines().any(|line| {
        line.trim()
            .strip_prefix("Terminal=")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
    });

    let argv = command_for(&exec, terminal)?;
    Some(App { name, id: id.to_owned(), exec: argv })
}

/// The argv for one `Exec=`. Pure: whether the program is installed is the
/// catalog's business, not the parser's.
fn command_for(exec: &str, terminal: bool) -> Option<Vec<String>> {
    let tokens = tokenize(exec);
    let first = tokens.first()?;
    let base = first.rsplit('/').next().unwrap_or(first);

    let mut argv: Vec<String> = if base == "flatpak" {
        // Rebuilt clean: the exported entry carries file-forwarding markers that
        // only make sense when files are actually passed.
        let id = tokens
            .iter()
            .skip_while(|token| token.as_str() != "run")
            .skip(1)
            .find(|token| !token.starts_with('-') && token.contains('.'))?;
        vec!["flatpak".to_owned(), "run".to_owned(), id.clone()]
    } else {
        let mut rest: Vec<String> = tokens.clone();
        if base == "env" {
            rest.remove(0);
            while rest.first().is_some_and(|token| token.contains('=') && !token.starts_with('-')) {
                rest.remove(0);
            }
        }
        rest
    };

    // Field codes are for the launcher to fill in, and there is no launcher.
    argv = argv.iter().map(|token| strip_codes(token)).filter(|token| !token.is_empty()).collect();

    if argv.is_empty() {
        return None;
    }

    let program = argv[0].clone();
    let base = program.rsplit('/').next().unwrap_or(&program);
    if NOT_AN_APP.contains(&base) {
        return None;
    }
    // A terminal entry without a terminal dies with an error nobody sees. The
    // catalog drops it later if no terminal is installed.
    if terminal {
        argv.insert(0, "kitty".to_owned());
    }

    Some(argv)
}

fn program_exists(program: &str) -> bool {
    if program.starts_with('/') {
        return Path::new(program).is_file();
    }
    find_in_path(program).is_some()
}

fn find_in_path(program: &str) -> Option<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.display().to_string())
}

/// `--foo %U` and `%F` both disappear; `%%` becomes a literal `%`.
fn strip_codes(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    let mut chars = token.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        match chars.peek().copied() {
            Some('%') => {
                chars.next();
                out.push('%');
            }
            Some('f' | 'F' | 'u' | 'U' | 'i' | 'c' | 'k' | 'd' | 'D' | 'n' | 'N' | 'v' | 'm') => {
                // The code is removed and nothing takes its place: there is no
                // launcher here to fill it in with a file list.
                chars.next();
            }
            _ => out.push('%'),
        }
    }
    if out.starts_with("--") && out.ends_with('=') {
        out.clear();
    }
    out
}

/// Splits an `Exec=` the way the desktop entry spec says: whitespace, with
/// double quotes grouping, and backslash escaping inside them.
fn tokenize(exec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut has = false;
    let mut chars = exec.chars();

    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                quoted = !quoted;
                has = true;
            }
            '\\' if quoted => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    has = true;
                }
            }
            ch if ch.is_whitespace() && !quoted => {
                if has {
                    tokens.push(std::mem::take(&mut current));
                    has = false;
                }
            }
            ch => {
                current.push(ch);
                has = true;
            }
        }
    }
    if has {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPOTIFY: &str = r#"[Desktop Entry]
Type=Application
Name=Spotify
Exec=/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=spotify --file-forwarding com.spotify.Client @@u %U @@
Icon=com.spotify.Client
"#;

    const NVIM: &str = r#"[Desktop Entry]
Type=Application
Name=Neovim
Exec=nvim %F
Terminal=true
"#;

    fn entry(text: &str, id: &str) -> App {
        parse_entry(text, id).expect("debería parsear")
    }

    #[test]
    fn a_flatpak_entry_is_rebuilt_without_file_forwarding_markers() {
        let app = entry(SPOTIFY, "com.spotify.Client");

        assert_eq!(app.name, "Spotify");
        assert_eq!(app.exec, vec!["flatpak", "run", "com.spotify.Client"]);
    }

    #[test]
    fn the_model_cannot_add_arguments_because_there_is_nowhere_to_put_them() {
        // `open_app` takes a name and nothing else; the argv is the file's.
        let app = entry(SPOTIFY, "com.spotify.Client");
        assert_eq!(app.exec.len(), 3);
        assert!(!app.exec.iter().any(|token| token.contains("--")));
    }

    #[test]
    fn placeholders_disappear_and_an_env_prefix_is_stripped() {
        let text = "[Desktop Entry]\nType=Application\nName=Tienda\n\
Exec=env DISABLE_MANGOHUD=1 /usr/bin/true %U\n";
        let app = entry(text, "tienda");

        assert_eq!(app.exec, vec!["/usr/bin/true"]);
    }

    #[test]
    fn a_hidden_entry_is_not_an_application() {
        let text = "[Desktop Entry]\nType=Application\nName=Oculta\nNoDisplay=true\nExec=/usr/bin/true\n";
        assert!(parse_entry(text, "oculta").is_none());
    }

    #[test]
    fn a_shell_is_not_an_application() {
        let text = "[Desktop Entry]\nType=Application\nName=Mala\nExec=/bin/sh -c \"rm -rf /\"\n";
        assert!(parse_entry(text, "mala").is_none());
    }

    #[test]
    fn the_exact_name_wins_over_a_name_that_contains_it() {
        let apps = vec![
            entry(SPOTIFY, "com.spotify.Client"),
            entry("[Desktop Entry]\nType=Application\nName=Spotify Radio\nExec=/usr/bin/true\n", "radio"),
        ];

        match resolve(&apps, "spotify") {
            Lookup::One(app) => assert_eq!(app.id, "com.spotify.Client"),
            other => panic!("esperaba una sola: {other:?}"),
        }
    }

    #[test]
    fn two_equally_good_names_ask_instead_of_guessing() {
        let apps = vec![
            entry("[Desktop Entry]\nType=Application\nName=Editor Uno\nExec=/usr/bin/true\n", "uno"),
            entry("[Desktop Entry]\nType=Application\nName=Editor Dos\nExec=/usr/bin/true\n", "dos"),
        ];

        match resolve(&apps, "editor") {
            Lookup::Many(found) => assert_eq!(found.len(), 2),
            other => panic!("esperaba varias: {other:?}"),
        }
    }

    #[test]
    fn an_unknown_name_finds_nothing() {
        let apps = vec![entry(SPOTIFY, "com.spotify.Client")];
        assert_eq!(resolve(&apps, "photoshop"), Lookup::None);
    }

    #[test]
    fn accents_do_not_matter_when_he_speaks() {
        let apps = vec![entry(
            "[Desktop Entry]\nType=Application\nName=Configuración\nExec=/usr/bin/true\n",
            "config",
        )];
        assert!(matches!(resolve(&apps, "configuracion"), Lookup::One(_)));
    }

    /// Not part of the suite: it reads the real desktop of whatever machine runs
    /// it, so CI would fail for the wrong reason. Run it by hand when a name
    /// does not resolve: `cargo test -p nerea-core --lib the_catalog_of_this_machine
    /// -- --ignored --nocapture`.
    #[test]
    #[ignore = "lee el escritorio real de esta máquina"]
    fn the_catalog_of_this_machine() {
        let apps = catalog();
        assert!(!apps.is_empty(), "el catálogo no puede estar vacío");
        for app in &apps {
            eprintln!("{} -> {}", app.label(), app.exec.join(" "));
        }

        match resolve(&apps, "spotify") {
            Lookup::One(app) => eprintln!("spotify resuelve a: {}", app.exec.join(" ")),
            other => panic!("spotify no resolvió: {other:?}"),
        }
    }

    #[test]
    fn a_terminal_entry_gets_a_terminal() {
        assert_eq!(entry(NVIM, "nvim").exec, vec!["kitty", "nvim"]);
    }
}
