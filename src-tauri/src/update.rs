//! Updating from the repository checkout.
//!
//! The app is developed and run from a checkout on this machine, so "update"
//! means: pull the branch, rebuild the release binary, and start it again.
//! A packaged build has no checkout, and reports that honestly instead of
//! pretending to update.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde::Serialize;

/// What the UI needs to decide whether to offer the menu entry.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    pub current: String,
    pub repo: Option<PathBuf>,
    pub branch: String,
    pub upstream: String,
    pub behind: usize,
    pub dirty: bool,
    /// True only when a pull would be safe and something is waiting.
    pub can_update: bool,
}

impl UpdateStatus {
    fn unavailable() -> Self {
        Self {
            current: env!("CARGO_PKG_VERSION").to_owned(),
            repo: None,
            branch: String::new(),
            upstream: String::new(),
            behind: 0,
            dirty: false,
            can_update: false,
        }
    }
}

/// The checkout this binary belongs to, if any.
fn repo_root() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors().skip(1) {
            if ancestor.join(".git").exists() {
                return Some(ancestor.to_path_buf());
            }
        }
    }

    let built = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
    built.join(".git").exists().then(|| built.to_path_buf())
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).current_dir(repo).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Full stderr when a command fails, so the panel can show why.
fn explain(output: &std::process::Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if text.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    } else {
        text
    }
}

pub fn status() -> UpdateStatus {
    let Some(repo) = repo_root() else {
        return UpdateStatus::unavailable();
    };

    // Best effort: without a network the local information still helps.
    let _ = git(&repo, &["fetch", "--quiet", "--prune"]);

    match git(&repo, &["status", "--porcelain=v1", "--branch"]) {
        Some(output) => parse(&output, repo),
        None => UpdateStatus::unavailable(),
    }
}

/// Parses `git status --porcelain=v1 --branch`.
///
/// The first line is the branch header (`## main...origin/main [behind 2]`);
/// every other non-empty line is a change in the working tree.
fn parse(output: &str, repo: PathBuf) -> UpdateStatus {
    let mut lines = output.lines();
    let header = lines.next().unwrap_or_default().trim_start_matches("## ");

    let behind = header
        .find("behind ")
        .and_then(|start| {
            header[start + 7..]
                .split(|character: char| !character.is_ascii_digit())
                .find(|part| !part.is_empty())
        })
        .and_then(|count| count.parse::<usize>().ok())
        .unwrap_or(0);

    let (branch, upstream) = match header.split_once("...") {
        Some((branch, rest)) => (
            branch.to_owned(),
            rest.split_whitespace().next().unwrap_or_default().to_owned(),
        ),
        None => (header.split_whitespace().next().unwrap_or_default().to_owned(), String::new()),
    };

    let dirty = lines.any(|line| !line.trim().is_empty());

    UpdateStatus {
        current: env!("CARGO_PKG_VERSION").to_owned(),
        repo: Some(repo),
        branch,
        upstream,
        behind,
        dirty,
        can_update: behind > 0 && !dirty,
    }
}

/// Fast half of the update: refuses on a dirty tree, then fast-forwards.
pub fn pull() -> Result<String, String> {
    let repo = repo_root().ok_or("this build has no checkout to update from")?;

    if status().dirty {
        return Err("hay cambios sin commitear en el repo; no toco nada".to_owned());
    }

    let pull = Command::new("git")
        .args(["pull", "--ff-only"])
        .current_dir(&repo)
        .output()
        .map_err(|err| err.to_string())?;
    if !pull.status.success() {
        return Err(format!("git pull falló: {}", explain(&pull)));
    }

    Ok(String::from_utf8_lossy(&pull.stdout).trim().to_owned())
}

/// Slow half: rebuild the release binary. A failure leaves the old one in place.
pub fn build() -> Result<(), String> {
    let repo = repo_root().ok_or("this build has no checkout to update from")?;

    let build = Command::new("cargo")
        .args(["build", "--release", "-p", "nerea-desktop"])
        .current_dir(&repo)
        .output()
        .map_err(|err| err.to_string())?;
    if !build.status.success() {
        return Err(format!("cargo build falló: {}", explain(&build)));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_of(output: &str) -> UpdateStatus {
        parse(output, PathBuf::from("/tmp/nerea"))
    }

    #[test]
    fn a_clean_branch_behind_upstream_can_update() {
        let status = status_of("## main...origin/main [behind 3]\n");
        assert_eq!(status.branch, "main");
        assert_eq!(status.upstream, "origin/main");
        assert_eq!(status.behind, 3);
        assert!(!status.dirty);
        assert!(status.can_update);
    }

    #[test]
    fn a_dirty_tree_blocks_the_update() {
        let status = status_of("## main...origin/main [behind 2]\n M src-tauri/src/lib.rs\n");
        assert_eq!(status.behind, 2);
        assert!(status.dirty);
        assert!(!status.can_update);
    }

    #[test]
    fn up_to_date_reports_nothing_to_do() {
        let status = status_of("## main...origin/main\n");
        assert_eq!(status.behind, 0);
        assert!(!status.can_update);
    }

    #[test]
    fn a_branch_without_upstream_does_not_crash() {
        let status = status_of("## feature\n");
        assert_eq!(status.branch, "feature");
        assert_eq!(status.upstream, "");
        assert_eq!(status.behind, 0);
        assert!(!status.can_update);
    }

    #[test]
    fn ahead_and_behind_together_only_counts_behind() {
        let status = status_of("## main...origin/main [ahead 1, behind 4]\n");
        assert_eq!(status.behind, 4);
        assert!(status.can_update);
    }
}
