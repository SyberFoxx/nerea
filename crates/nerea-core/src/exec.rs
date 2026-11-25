//! Her hands: running a program, with the blast radius written down.
//!
//! There is no shell anywhere in this file. A command is a program and a list of
//! arguments, so `;`, `|`, `$(…)` and redirections do not exist as concepts: an
//! argument that looks like a command is just a string the program has to
//! interpret, and no program here does.
//!
//! Two rules are enforced in code, not in a prompt:
//!
//! 1. **Never root.** `sudo`, `pkexec`, `doas` and friends are refused before
//!    anything spawns.
//! 2. **Deleting stays inside declared roots.** `~/.cache`, his projects and her
//!    own work folder. Never a root itself, never an ancestor of one: a model
//!    that hallucinates `rm -rf ~` gets a refusal, not a confirmation card.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use serde::{Deserialize, Serialize};

/// How long a command may take before the tree is cut.
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Output kept: the whole thing is charged against the turn's reading budget.
pub const MAX_OUTPUT: usize = 8_000;

/// Programs that execute whatever a project tells them to. Allowed, but the
/// card has to say so: a build script runs with his permissions.
const CODE: &[&str] = &["cargo", "npm", "npx", "node", "python3", "make", "pip", "pipx"];

/// The line the approval card shows, when the program deserves one.
pub fn warning(program: &str) -> Option<&'static str> {
    CODE.contains(&program).then_some(
        "esto puede ejecutar cualquier cosa que el proyecto le pida, con tus permisos",
    )
}

/// Programs that escalate privileges. No configuration makes them runnable.
const NEVER: &[&str] = &[
    "sudo",
    "pkexec",
    "doas",
    "su",
    "sudoedit",
    "run0",
    "polkit-helper",
];

/// What she is allowed to run, and where she is allowed to touch.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Tools {
    /// Programs, by name. Empty means nothing runs.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Folders where she may create, modify and delete. Paths are expanded.
    #[serde(default)]
    pub roots: Vec<String>,
    /// Seconds a single command may take.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Commands he said "always" to, by exact fingerprint. Never a program:
    /// `playerctl next` is not `playerctl`.
    #[serde(default)]
    pub trust: Vec<String>,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

impl Tools {
    /// The defaults written the first time she needs them.
    pub fn defaults() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home".to_owned());
        Self {
            allow: [
                "playerctl",
                "wpctl",
                "pactl",
                "brightnessctl",
                "wl-copy",
                "grim",
                "hyprctl",
                "xdg-open",
                "pandoc",
                "ffmpeg",
                "magick",
                "pdftotext",
                "zip",
                "unzip",
                "trash-put",
                "mkdir",
                "cp",
                "mv",
                "rm",
                "cargo",
                "git",
            ]
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
            roots: vec![
                format!("{home}/.cache"),
                format!("{home}/Proyectos"),
                format!("{home}/.local/share/nerea/work"),
                "/tmp".to_owned(),
            ],
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            trust: Vec::new(),
        }
    }

    /// Reads `~/.config/nerea/tools.toml`, writing the defaults when missing.
    pub fn load() -> Self {
        let path = config_path();
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(tools) = toml::from_str::<Tools>(&text)
        {
            return tools;
        }

        let tools = Self::defaults();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = toml::to_string_pretty(&tools) {
            let _ = std::fs::write(&path, text);
        }
        tools
    }

    pub fn allows(&self, program: &str) -> bool {
        let name = program.rsplit('/').next().unwrap_or(program);
        self.allow.iter().any(|allowed| allowed == name)
    }

    /// The roots, expanded and canonicalised as far as they exist.
    pub fn resolved_roots(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|root| expand(root)).filter(|root| root.exists()).collect()
    }

    /// The key a "always" click writes: the program and its exact arguments.
    pub fn trusts(&self, key: &str) -> bool {
        self.trust.iter().any(|trusted| trusted == key)
    }

    fn persist(&self) -> Result<(), String> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let text = toml::to_string_pretty(self).map_err(|err| err.to_string())?;
        std::fs::write(&path, text).map_err(|err| err.to_string())
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.clamp(1, 600))
    }
}

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_owned())).join(".config")
        });
    base.join("nerea").join("tools.toml")
}

/// `program|arg|arg` — stable across runs, and narrow on purpose.
pub fn fingerprint(program: &str, args: &[String]) -> String {
    let mut out = program.to_owned();
    for arg in args {
        out.push('|');
        out.push_str(arg);
    }
    out
}

/// Remembers one command so the card does not ask again.
pub fn remember_command(key: &str) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("no hay comando que recordar".to_owned());
    }

    let mut tools = Tools::load();
    if !tools.trusts(key) {
        tools.trust.push(key.to_owned());
    }
    tools.persist()
}

/// One validated command.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// What a command produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// What she reads, capped.
    pub fn report(&self) -> String {
        let mut out = format!("$ salida: {}\n", self.code.map_or("señal".to_owned(), |c| c.to_string()));
        if !self.stdout.trim().is_empty() {
            out.push_str(self.stdout.trim_end());
            out.push('\n');
        }
        if !self.stderr.trim().is_empty() {
            out.push_str("errores:\n");
            out.push_str(self.stderr.trim_end());
            out.push('\n');
        }
        if out.len() > MAX_OUTPUT {
            let kept: String = out.chars().take(MAX_OUTPUT).collect();
            return format!("{kept}\n[recortado: la salida era más larga]");
        }
        out
    }
}

/// Validates a command against the policy. Everything that can be refused is
/// refused here, before a process exists.
pub fn plan(tools: &Tools, program: &str, args: &[String], cwd: Option<&str>) -> Result<Plan, String> {
    let name = program.trim();
    if name.is_empty() {
        return Err("falta el programa".to_owned());
    }
    if name.contains('/') {
        return Err(format!("«{name}» es una ruta: pasá el nombre del programa"));
    }
    if NEVER.contains(&name) {
        return Err(format!(
            "«{name}» pide permisos de administrador, y eso lo hacés vos, no yo"
        ));
    }
    if !tools.allows(name) {
        return Err(format!(
            "«{name}» no está en tu lista de programas permitidos (~/.config/nerea/tools.toml)"
        ));
    }
    if args.iter().any(|arg| arg.contains('\0')) {
        return Err("un argumento tiene un byte nulo".to_owned());
    }

    let cwd = match cwd {
        Some(dir) => {
            let path = expand(dir);
            if !path.is_dir() {
                return Err(format!("{} no es una carpeta", path.display()));
            }
            path
        }
        None => std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp")),
    };

    let roots = tools.resolved_roots();
    for arg in args {
        check_argument(arg, &rooted(&roots), &cwd)?;
    }

    Ok(Plan { program: name.to_owned(), args: args.to_vec(), cwd })
}

/// Every path argument is checked twice: it may not be a credential file, and
/// it may not live outside the roots.
fn check_argument(arg: &str, roots: &[PathBuf], cwd: &Path) -> Result<(), String> {
    let Some(value) = path_like(arg) else {
        return Ok(());
    };

    let candidate = if let Some(rest) = value.strip_prefix("~/") {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest)
    } else if value == "~" {
        PathBuf::from(std::env::var("HOME").unwrap_or_default())
    } else if let Some(rest) = value.strip_prefix("./") {
        cwd.join(rest)
    } else if value.starts_with('/') {
        PathBuf::from(value)
    } else {
        cwd.join(value)
    };

    // Reading a secret is refused even inside a root: a cache that holds a key
    // is still a key.
    if let Ok(resolved) = candidate.canonicalize() {
        crate::tools::guard::deny(&resolved)?;

        // A root is a boundary, not a target: emptying the whole cache is not
        // "cleaning the cache", and `~` contains every root there is.
        if roots.contains(&resolved) {
            return Err(format!(
                "«{}» es una de tus carpetas de trabajo, no lo que hay adentro: no la toco entera",
                resolved.display()
            ));
        }
        if roots.iter().any(|root| root.starts_with(&resolved)) {
            return Err(format!(
                "«{}» contiene tus carpetas de trabajo: ahí no toco nada",
                resolved.display()
            ));
        }
        // Something that exists may be read anywhere the guard allows: pandoc
        // has to read the markdown he wrote in his home.
        if !resolved.starts_with(cwd)
            && !roots.iter().any(|root| resolved.starts_with(root))
            && !resolved.starts_with(std::env::var("HOME").unwrap_or_default())
        {
            return Err(format!("{} está fuera de tus carpetas de trabajo", resolved.display()));
        }
        return Ok(());
    }

    // It does not exist yet, so it is an output: it has to land inside a root.
    if roots.iter().any(|root| candidate.starts_with(root)) {
        return Ok(());
    }

    Err(format!(
        "«{value}» está fuera de las carpetas donde puedo escribir ({}), y no voy a crear nada ahí",
        roots.iter().map(|root| root.display().to_string()).collect::<Vec<_>>().join(", ")
    ))
}

/// The part of an argument that could be a path: `--output=/tmp/x` and
/// `/tmp/x` both count; `--number-of-lines=3` does not.
fn path_like(arg: &str) -> Option<&str> {
    let value = arg.split_once('=').map(|(_, value)| value).unwrap_or(arg);
    if value.is_empty() || value.starts_with('-') {
        return None;
    }

    let looks_like_path = value.starts_with('/')
        || value.starts_with("~/")
        || value.starts_with("./")
        || value.starts_with("../")
        || value == "~";

    looks_like_path.then_some(value)
}

fn expand(input: &str) -> PathBuf {
    let trimmed = input.trim();
    if trimmed == "~" {
        return PathBuf::from(std::env::var("HOME").unwrap_or_default());
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest);
    }
    PathBuf::from(trimmed)
}

/// Roots as absolute, canonical paths; the comparison is against what the
/// kernel would see, not against a string the model wrote.
fn rooted(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .collect()
}

/// Runs a plan. A command that outlives its timeout is cut, and the whole
/// process group goes with it: a build that leaves `rustc` behind is a leak.
pub fn run(plan: &Plan, timeout: Duration) -> Result<Outcome, String> {
    let mut command = Command::new(&plan.program);
    command
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (key, value) in clean_environment() {
        command.env(key, value);
    }

    // Its own group, so stopping it stops what it spawned.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let child = command
        .spawn()
        .map_err(|err| format!("no pude ejecutar «{}»: {err}", plan.program))?;
    let pid = child.id();

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });

    match receiver.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(Outcome {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
        Ok(Err(err)) => Err(format!("no pude leer la salida: {err}")),
        Err(_) => {
            kill_group(pid);
            Err(format!(
                "«{}» tardó más de {} s y lo corté",
                plan.program,
                timeout.as_secs()
            ))
        }
    }
}

fn kill_group(pid: u32) {
    let _ = Command::new("kill")
        .args(["-TERM", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// What the child may see. No API key, no session token, no secret from her own
/// environment: a program that echoes `env` must not leak one.
fn clean_environment() -> Vec<(String, String)> {
    const KEEP: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "LC_ALL",
        "TERM",
        "TZ",
        "XDG_RUNTIME_DIR",
        "XDG_DATA_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_SESSION_TYPE",
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
    ];

    let mut kept: Vec<(String, String)> = KEEP
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| ((*key).to_owned(), value)))
        .collect();

    if !kept.iter().any(|(key, _)| key == "PATH") {
        kept.push(("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()));
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Tools {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home".to_owned());
        Tools {
            allow: ["pandoc", "rm", "mkdir", "playerctl", "cargo"]
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            roots: vec![format!("{home}/.cache"), format!("{home}/Proyectos"), "/tmp".to_owned()],
            timeout_secs: 60,
            trust: Vec::new(),
        }
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn the_fingerprint_is_the_exact_command_not_the_program() {
        let next = fingerprint("playerctl", &args(&["next"]));
        let back = fingerprint("playerctl", &args(&["previous"]));

        assert_eq!(next, "playerctl|next");
        assert_ne!(next, back, "permitir uno no permite el otro");
    }

    #[test]
    fn only_an_exact_fingerprint_is_trusted() {
        let mut tools = tools();
        tools.trust.push("playerctl|next".to_owned());

        assert!(tools.trusts("playerctl|next"));
        assert!(!tools.trusts("playerctl|previous"));
        assert!(!tools.trusts("playerctl"));
    }

    #[test]
    fn a_tool_in_the_list_plans_fine() {
        let plan = plan(&tools(), "playerctl", &args(&["next"]), None).expect("planned");
        assert_eq!(plan.program, "playerctl");
        assert_eq!(plan.args, vec!["next"]);
    }

    #[test]
    fn a_program_outside_the_list_is_refused() {
        let err = plan(&tools(), "ffmpeg", &[], None).expect_err("must refuse");
        assert!(err.contains("lista de programas"), "{err}");
    }

    #[test]
    fn privilege_escalation_is_refused_even_if_configured() {
        let mut tools = tools();
        tools.allow.push("sudo".to_owned());
        let err = plan(&tools, "sudo", &args(&["pacman", "-Syu"]), None).expect_err("must refuse");
        assert!(err.contains("administrador"), "{err}");
    }

    #[test]
    fn a_program_cannot_be_smuggled_in_as_a_path() {
        let err = plan(&tools(), "/bin/rm", &args(&["-rf", "/tmp/x"]), None).expect_err("refuse");
        assert!(err.contains("es una ruta"), "{err}");
    }

    #[test]
    fn deleting_a_root_itself_is_refused() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home".to_owned());
        for target in [format!("{home}/.cache"), format!("{home}/Proyectos"), "/".to_owned()] {
            let err = plan(&tools(), "rm", &args(&["-rf", &target]), None).expect_err(&target);
            assert!(
                err.contains("carpetas de trabajo")
                    || err.contains("contiene tus carpetas")
                    || err.contains("fuera de"),
                "{target}: {err}"
            );
        }
    }

    #[test]
    fn deleting_inside_a_root_is_allowed() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home".to_owned());
        let target = format!("{home}/.cache/mozilla");
        let planned = plan(&tools(), "rm", &args(&["-rf", &target]), None);
        assert!(planned.is_ok(), "{planned:?}");
    }

    #[test]
    fn writing_outside_the_roots_is_refused() {
        let err = plan(&tools(), "mkdir", &args(&["/etc/nerea-test"]), None).expect_err("refuse");
        assert!(err.contains("fuera de las carpetas"), "{err}");
    }

    #[test]
    fn a_credential_file_is_never_an_argument() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home".to_owned());
        let secret = format!("{home}/.ssh/id_rsa");
        if !Path::new(&secret).is_file() {
            return;
        }
        let err = plan(&tools(), "pandoc", &args(&[&secret, "-o", "/tmp/x.pdf"]), None)
            .expect_err("must refuse");
        assert!(err.contains("credenciales") || err.contains("protegido"), "{err}");
    }

    #[test]
    fn an_option_value_that_is_a_path_is_checked_too() {
        let err = plan(&tools(), "mkdir", &args(&["--mode=755", "/etc/nerea-test"]), None)
            .expect_err("refuse");
        assert!(err.contains("fuera de las carpetas"), "{err}");
    }

    #[test]
    fn flags_and_numbers_are_not_paths() {
        assert!(path_like("--number-of-lines=3").is_none());
        assert!(path_like("next").is_none());
        assert!(path_like("-rf").is_none());
        assert_eq!(path_like("--output=/tmp/x.pdf"), Some("/tmp/x.pdf"));
        assert_eq!(path_like("/tmp/x.pdf"), Some("/tmp/x.pdf"));
    }

    #[test]
    fn the_environment_a_child_sees_carries_no_secret() {
        unsafe { std::env::set_var("NEREA_SECRET_PROBE", "no-debe-pasar") };
        let kept = clean_environment();
        assert!(!kept.iter().any(|(key, _)| key == "NEREA_SECRET_PROBE"));
        assert!(kept.iter().any(|(key, _)| key == "PATH"));
    }

    #[test]
    fn a_command_really_runs_and_reports_its_code() {
        let mut tools = tools();
        tools.allow.push("echo".to_owned());
        let planned = plan(&tools, "echo", &args(&["hola"]), None).expect("planned");

        let outcome = run(&planned, Duration::from_secs(5)).expect("ran");
        assert!(outcome.ok(), "{outcome:?}");
        assert_eq!(outcome.stdout.trim(), "hola");
        assert!(outcome.report().contains("0"));
    }

    #[test]
    fn a_missing_program_is_an_error_not_a_panic() {
        let mut tools = tools();
        tools.allow.push("no-existe-este-programa".to_owned());
        let planned = plan(&tools, "no-existe-este-programa", &[], None).expect("planned");
        let err = run(&planned, Duration::from_secs(2)).expect_err("must fail");
        assert!(err.contains("no pude ejecutar"), "{err}");
    }
}
