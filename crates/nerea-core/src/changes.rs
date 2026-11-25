//! Changes she proposes, and the diff he reads before saying yes.
//!
//! Two phases on purpose. A tool call stages a proposal and touches nothing: it
//! writes one JSON file inside her own data directory. The app shows the diff.
//! Only an explicit approval writes the target, and the previous content is
//! copied first — a wrong keystroke has to cost nothing.
//!
//! The model can never write by itself, not even if it asks nicely: there is no
//! code path from a tool call to `apply`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tools::guard;

/// Nothing bigger than this is proposed, read or written: a patch is a patch.
pub const MAX_BYTES: usize = 512 * 1024;
/// Lines kept on each side of the diff, so one huge file cannot eat the screen.
const MAX_DIFF_LINES: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Create,
    Overwrite,
    Edit,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Create => "nuevo",
            Kind::Overwrite => "reemplaza",
            Kind::Edit => "edita",
        }
    }
}

/// One proposed change, as it waits for approval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    pub path: String,
    pub kind: Kind,
    pub at: u64,
    /// Empty when the file does not exist yet.
    #[serde(default)]
    pub before: String,
    pub after: String,
}

impl Change {
    /// `(added, removed)` lines, for the one-line summary on the card.
    pub fn counts(&self) -> (usize, usize) {
        let mut added = 0;
        let mut removed = 0;
        for line in diff(&self.before, &self.after) {
            match line.kind {
                DiffKind::Added => added += 1,
                DiffKind::Removed => removed += 1,
                DiffKind::Same => {}
            }
        }
        (added, removed)
    }

    /// The diff, ready to print: unified enough to read, short enough to fit.
    pub fn preview(&self) -> String {
        let mut out = String::new();

        for (shown, line) in diff(&self.before, &self.after).iter().enumerate() {
            if shown >= MAX_DIFF_LINES {
                out.push_str("\n[recortado: el cambio es más largo]");
                break;
            }
            let marker = match line.kind {
                DiffKind::Added => '+',
                DiffKind::Removed => '-',
                DiffKind::Same => ' ',
            };
            out.push(marker);
            out.push_str(&line.text);
            out.push('\n');
        }

        if out.is_empty() {
            out.push_str("(sin cambios)");
        }
        out
    }
}

/// Where proposals wait. Inside her data dir, never next to the target.
pub fn dir() -> PathBuf {
    crate::paths::data_dir().join("pending")
}

/// Proposes writing `content` to `path` — whole file, new or replaced.
pub fn stage(path: &str, content: &str) -> Result<Change, String> {
    stage_in(&dir(), path, content)
}

/// The same, with the proposal folder injected: tests must not share an
/// environment variable, and an environment variable is not a parameter.
pub fn stage_in(pending: &Path, path: &str, content: &str) -> Result<Change, String> {
    let target = target(path)?;
    if content.len() > MAX_BYTES {
        return Err(format!(
            "el contenido pesa {} KB y el límite es {} KB",
            content.len() / 1024,
            MAX_BYTES / 1024
        ));
    }

    let before = match std::fs::read(&target) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| {
            format!("{} no es texto: no lo toco", target.display())
        })?,
        Err(_) => String::new(),
    };

    let kind = if before.is_empty() { Kind::Create } else { Kind::Overwrite };
    let change = build(target, kind, before, content.to_owned())?;
    save_in(pending, &change)?;
    Ok(change)
}

/// Proposes replacing `find` with `replace` inside an existing file.
///
/// `find` has to appear exactly once: an edit that matches twice is an edit
/// nobody reviewed.
pub fn edit(path: &str, find: &str, replace: &str) -> Result<Change, String> {
    edit_in(&dir(), path, find, replace)
}

pub fn edit_in(pending: &Path, path: &str, find: &str, replace: &str) -> Result<Change, String> {
    if find.trim().is_empty() {
        return Err("falta el texto a reemplazar".to_owned());
    }

    let target = target(path)?;
    let before = std::fs::read_to_string(&target)
        .map_err(|err| format!("no puedo leer {}: {err}", target.display()))?;

    let matches = before.matches(find).count();
    match matches {
        0 => return Err(format!("«{}» no aparece en el archivo", shorten(find, 60))),
        1 => {}
        many => {
            return Err(format!(
                "«{}» aparece {many} veces: necesito un fragmento que aparezca una sola vez",
                shorten(find, 60)
            ));
        }
    }

    if replace.len() > MAX_BYTES {
        return Err("el reemplazo es demasiado grande".to_owned());
    }

    let after = before.replacen(find, replace, 1);
    let change = build(target, Kind::Edit, before, after)?;
    save_in(pending, &change)?;
    Ok(change)
}

/// Every proposal waiting, newest first.
pub fn list() -> Vec<Change> {
    list_in(&dir())
}

pub fn list_in(pending: &Path) -> Vec<Change> {
    let Ok(entries) = std::fs::read_dir(pending) else {
        return Vec::new();
    };

    let mut changes: Vec<Change> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "json"))
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str::<Change>(&text).ok())
        .collect();

    changes.sort_by_key(|change| std::cmp::Reverse(change.at));
    changes
}

pub fn load(id: &str) -> Option<Change> {
    load_in(&dir(), id)
}

pub fn load_in(pending: &Path, id: &str) -> Option<Change> {
    let text = std::fs::read_to_string(pending.join(format!("{}.json", safe_id(id)?))).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes the change for real: backup first, then an atomic rename.
pub fn apply(id: &str) -> Result<String, String> {
    apply_in(&dir(), id)
}

pub fn apply_in(pending: &Path, id: &str) -> Result<String, String> {
    let change = load_in(pending, id).ok_or("esa propuesta ya no existe")?;
    let target = target(&change.path)?;
    if change.after.len() > MAX_BYTES {
        return Err("el cambio es demasiado grande".to_owned());
    }

    // The file may have moved under us since it was proposed.
    let current = std::fs::read(&target).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
    if current != change.before {
        return Err(format!(
            "{} cambió desde que se propuso el cambio: descartalo y pedímelo de nuevo",
            target.display()
        ));
    }

    if !change.before.is_empty() {
        let backups = pending.with_file_name("backups");
        std::fs::create_dir_all(&backups)
            .map_err(|err| format!("no puedo preparar el respaldo: {err}"))?;
        let name = target.file_name().and_then(|name| name.to_str()).unwrap_or("archivo");
        std::fs::write(backups.join(format!("{}-{name}", change.at)), change.before.as_bytes())
            .map_err(|err| format!("no puedo respaldar el original: {err}"))?;
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("no puedo crear {}: {err}", parent.display()))?;
    }

    let temp = target.with_extension("nerea-tmp");
    std::fs::write(&temp, change.after.as_bytes())
        .map_err(|err| format!("no pude escribir: {err}"))?;
    std::fs::rename(&temp, &target).map_err(|err| format!("no pude guardar: {err}"))?;

    let _ = std::fs::remove_file(pending.join(format!("{}.json", id)));

    Ok(format!("Listo: escribí {}", target.display()))
}

pub fn discard(id: &str) -> Result<(), String> {
    discard_in(&dir(), id)
}

pub fn discard_in(pending: &Path, id: &str) -> Result<(), String> {
    let file = pending.join(format!("{}.json", safe_id(id).ok_or("id inválido")?));
    std::fs::remove_file(file).map_err(|err| format!("no pude descartar: {err}"))
}

/// Resolves a path a human wrote, refusing everything the guard refuses.
///
/// The file may not exist yet, so the parent is what gets canonicalised: that is
/// where a symlink would hide.
fn target(input: &str) -> Result<PathBuf, String> {
    let path = guard::resolve_for_write(input)?;

    if path.is_dir() {
        return Err(format!("{} es una carpeta, no un archivo", path.display()));
    }
    if !path.exists() && path.parent().is_none() {
        return Err("esa ruta no tiene dónde vivir".to_owned());
    }

    Ok(path)
}

fn build(target: PathBuf, kind: Kind, before: String, after: String) -> Result<Change, String> {
    let path = target.to_string_lossy().to_string();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);

    Ok(Change { id: format!("{at}"), path, kind, at, before, after })
}

fn save_in(pending: &Path, change: &Change) -> Result<(), String> {
    std::fs::create_dir_all(pending)
        .map_err(|err| format!("no puedo guardar la propuesta: {err}"))?;
    let text = serde_json::to_string(change).map_err(|err| err.to_string())?;
    // Written beside and renamed: a half-written proposal is an unreadable one.
    let temp = pending.join(format!("{}.tmp", change.id));
    std::fs::write(&temp, text).map_err(|err| format!("no puedo guardar la propuesta: {err}"))?;
    std::fs::rename(&temp, pending.join(format!("{}.json", change.id)))
        .map_err(|err| format!("no puedo guardar la propuesta: {err}"))
}

/// Ids come back as JSON from the page; they are timestamps and nothing else.
fn safe_id(id: &str) -> Option<&str> {
    let trimmed = id.trim();
    (!trimmed.is_empty()
        && trimmed.len() <= 32
        && trimmed.chars().all(|character| character.is_ascii_digit()))
    .then_some(trimmed)
}

fn shorten(text: &str, max: usize) -> String {
    let flat = text.replace('\n', "\\n");
    if flat.chars().count() <= max {
        return flat;
    }
    format!("{}…", flat.chars().take(max).collect::<String>())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Same,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: DiffKind,
    pub text: String,
}

/// A line diff, long enough for a patch and short enough to read.
///
/// Plain LCS: correct, and the line counts here are capped by the caller.
pub fn diff(before: &str, after: &str) -> Vec<Line> {
    let old: Vec<&str> = before.lines().take(MAX_DIFF_LINES).collect();
    let new: Vec<&str> = after.lines().take(MAX_DIFF_LINES).collect();

    let rows = old.len() + 1;
    let columns = new.len() + 1;
    let mut table = vec![0usize; rows * columns];

    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i * columns + j] = if old[i] == new[j] {
                table[(i + 1) * columns + j + 1] + 1
            } else {
                table[(i + 1) * columns + j].max(table[i * columns + j + 1])
            };
        }
    }

    let mut lines = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            lines.push(Line { kind: DiffKind::Same, text: old[i].to_owned() });
            i += 1;
            j += 1;
        } else if table[(i + 1) * columns + j] >= table[i * columns + j + 1] {
            lines.push(Line { kind: DiffKind::Removed, text: old[i].to_owned() });
            i += 1;
        } else {
            lines.push(Line { kind: DiffKind::Added, text: new[j].to_owned() });
            j += 1;
        }
    }
    while i < old.len() {
        lines.push(Line { kind: DiffKind::Removed, text: old[i].to_owned() });
        i += 1;
    }
    while j < new.len() {
        lines.push(Line { kind: DiffKind::Added, text: new[j].to_owned() });
        j += 1;
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch world: the proposals live in `<base>/pending`, the targets
    /// live beside it. Nothing here reads an environment variable.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nerea-changes-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pending")).expect("scratch");
        dir
    }

    fn pending(base: &Path) -> PathBuf {
        base.join("pending")
    }

    #[test]
    fn a_new_file_is_proposed_not_written() {
        let base = scratch("create");
        let target = base.join("nota.txt");

        let change = stage_in(&pending(&base), target.to_str().expect("path"), "hola\n")
            .expect("proposed");

        assert_eq!(change.kind, Kind::Create);
        assert!(change.before.is_empty());
        assert!(!target.exists(), "la propuesta no toca el archivo");
        assert_eq!(list_in(&pending(&base)).len(), 1);
    }

    #[test]
    fn applying_writes_the_file_and_keeps_the_original() {
        let base = scratch("apply");
        let target = base.join("nota.txt");
        std::fs::write(&target, "viejo\n").expect("write");

        let change = stage_in(&pending(&base), target.to_str().expect("path"), "nuevo\n")
            .expect("proposed");
        let said = apply_in(&pending(&base), &change.id).expect("applied");

        assert_eq!(std::fs::read_to_string(&target).expect("read"), "nuevo\n");
        assert!(said.contains("escribí"), "{said}");

        let backups = base.join("backups");
        let kept: Vec<_> = std::fs::read_dir(&backups).expect("backups").flatten().collect();
        assert_eq!(kept.len(), 1);
        assert!(std::fs::read_to_string(kept[0].path()).expect("backup").contains("viejo"));
        assert!(list_in(&pending(&base)).is_empty(), "la propuesta se consume");
    }

    #[test]
    fn an_edit_that_matches_twice_is_refused() {
        let base = scratch("edit-twice");
        let target = base.join("codigo.txt");
        std::fs::write(&target, "a\nhola\nb\nhola\n").expect("write");

        let err = edit_in(&pending(&base), target.to_str().expect("path"), "hola", "chau")
            .expect_err("must refuse");

        assert!(err.contains("2 veces"), "{err}");
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "a\nhola\nb\nhola\n");
    }

    #[test]
    fn a_single_match_edits_exactly_one_line() {
        let base = scratch("edit-one");
        let target = base.join("codigo.txt");
        std::fs::write(&target, "a\nhola\nb\n").expect("write");

        let change = edit_in(&pending(&base), target.to_str().expect("path"), "hola", "chau")
            .expect("proposed");

        assert_eq!(change.kind, Kind::Edit);
        assert_eq!(change.after, "a\nchau\nb\n");
        assert_eq!(change.counts(), (1, 1));
        assert!(change.preview().contains("-hola"), "{}", change.preview());
        assert!(change.preview().contains("+chau"), "{}", change.preview());
    }

    #[test]
    fn a_proposal_for_a_file_that_changed_is_refused() {
        let base = scratch("stale");
        let target = base.join("nota.txt");
        std::fs::write(&target, "uno\n").expect("write");

        let change = stage_in(&pending(&base), target.to_str().expect("path"), "dos\n")
            .expect("proposed");
        std::fs::write(&target, "otra cosa\n").expect("change under us");

        let err = apply_in(&pending(&base), &change.id).expect_err("must refuse");
        assert!(err.contains("cambió"), "{err}");
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "otra cosa\n");
    }

    #[test]
    fn the_guard_still_refuses_a_protected_path() {
        let base = scratch("guard");
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_owned());
        let ssh = format!("{home}/.ssh/config");

        let err = stage_in(&pending(&base), &ssh, "Host *\n").expect_err("must refuse");
        assert!(err.contains("protegido") || err.contains("credenciales"), "{err}");
    }

    #[test]
    fn a_binary_target_is_never_rewritten() {
        let base = scratch("binary");
        let target = base.join("imagen.png");
        std::fs::write(&target, [0x89u8, b'P', b'N', b'G', 0]).expect("write");

        let err = stage_in(&pending(&base), target.to_str().expect("path"), "texto")
            .expect_err("must refuse");
        assert!(err.contains("no es texto"), "{err}");
    }

    #[test]
    fn discarding_leaves_the_world_alone() {
        let base = scratch("discard");
        let target = base.join("nota.txt");
        let change = stage_in(&pending(&base), target.to_str().expect("path"), "x\n")
            .expect("proposed");

        discard_in(&pending(&base), &change.id).expect("discarded");
        assert!(list_in(&pending(&base)).is_empty());
        assert!(!target.exists());
    }

    #[test]
    fn the_diff_is_a_diff() {
        let lines = diff("uno\ndos\ntres\n", "uno\nDOS\ntres\n");
        let rendered: String = lines
            .iter()
            .map(|line| {
                let marker = match line.kind {
                    DiffKind::Added => '+',
                    DiffKind::Removed => '-',
                    DiffKind::Same => ' ',
                };
                format!("{marker}{}\n", line.text)
            })
            .collect();

        assert_eq!(rendered, " uno\n-dos\n+DOS\n tres\n");
    }

    #[test]
    fn an_id_that_is_not_a_number_is_never_a_path() {
        assert!(safe_id("1755617972040").is_some());
        for bad in ["", "../x", "12/../../etc", "abc", "1 2"] {
            assert!(safe_id(bad).is_none(), "{bad}");
        }
    }
}
