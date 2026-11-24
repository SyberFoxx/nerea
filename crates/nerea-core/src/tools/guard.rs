//! What she may read, and what she may never read.
//!
//! The rules run *after* the path is resolved, so a symlink cannot walk her
//! around them.

use std::path::{Path, PathBuf};

/// Names that are not read even when they live somewhere allowed.
const SECRET_NAMES: &[&str] =
    &["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", ".netrc", ".pgpass", ".env", "credentials"];

const SECRET_SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".keystore", ".jks", ".kdbx"];

/// Expands `~`, resolves symlinks and then applies the rules.
pub fn resolve(input: &str) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("falta la ruta".to_owned());
    }

    let expanded = expand(trimmed);
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir().map_err(|err| err.to_string())?.join(expanded)
    };

    let path = absolute
        .canonicalize()
        .map_err(|_| format!("no encuentro «{trimmed}»"))?;
    deny(&path)?;
    Ok(path)
}

/// The rules, on an already resolved path.
pub fn deny(path: &Path) -> Result<(), String> {
    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        let lower = name.to_lowercase();
        if SECRET_NAMES.contains(&lower.as_str())
            || SECRET_SUFFIXES.iter().any(|suffix| lower.ends_with(suffix))
        {
            return Err(format!("«{name}» guarda credenciales: no lo abro"));
        }
    }

    for root in protected_roots() {
        if path == root || path.starts_with(&root) {
            return Err(format!("«{}» está protegido: no lo abro", path.display()));
        }
    }

    Ok(())
}

fn expand(input: &str) -> PathBuf {
    if input == "~" {
        return home();
    }
    if let Some(rest) = input.strip_prefix("~/") {
        return home().join(rest);
    }
    PathBuf::from(input)
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn protected_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = ["/etc/shadow", "/etc/sudoers", "/etc/sudoers.d", "/proc", "/sys"]
        .iter()
        .map(PathBuf::from)
        .collect();

    let home = home();
    for relative in [
        ".ssh",
        ".gnupg",
        ".pki",
        ".mozilla",
        ".password-store",
        ".config/google-chrome",
        ".config/chromium",
        ".config/BraveSoftware",
        ".config/nerea",
        ".local/share/nerea",
        ".local/share/keyrings",
    ] {
        roots.push(home.join(relative));
    }

    // Her own data dir, wherever `NEREA_DATA_DIR` put it. A relative fallback
    // would make every path look protected, so it only counts when absolute.
    let data = crate::paths::data_dir();
    if data.is_absolute() {
        roots.push(data);
    }

    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nerea-guard-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_secret_name_is_refused_even_in_a_friendly_directory() {
        let dir = scratch("env");
        let env = dir.join(".env");
        fs::write(&env, "API_KEY=live").expect("write");

        let resolved = env.canonicalize().expect("canonical");
        let err = deny(&resolved).expect_err("must refuse");
        assert!(err.contains("credenciales"), "{err}");
    }

    #[test]
    fn a_pem_key_is_refused() {
        let dir = scratch("pem");
        let key = dir.join("server.pem");
        fs::write(&key, "-----BEGIN").expect("write");

        assert!(deny(&key.canonicalize().expect("canonical")).is_err());
    }

    #[test]
    fn the_ssh_directory_is_never_readable() {
        let ssh = home().join(".ssh/id_rsa");
        assert!(deny(&ssh).is_err(), "the ssh dir has to be off limits");
    }

    #[test]
    fn an_ordinary_source_file_is_fine() {
        let dir = scratch("src");
        let file = dir.join("main.rs");
        fs::write(&file, "fn main() {}").expect("write");

        let resolved = resolve(file.to_str().expect("utf8")).expect("allowed");
        assert_eq!(resolved, file.canonicalize().expect("canonical"));
    }

    #[test]
    fn a_symlink_cannot_smuggle_a_secret_out() {
        let dir = scratch("link");
        let link = dir.join("innocent.txt");
        let target = home().join(".ssh/id_rsa");

        // The name rule covers `.env` above; this one asserts the resolution
        // order, so it only runs where there is a key to point at.
        if !target.is_file() || std::os::unix::fs::symlink(&target, &link).is_err() {
            return;
        }

        let err = resolve(link.to_str().expect("utf8")).expect_err("must refuse");
        assert!(err.contains("protegido") || err.contains("credenciales"), "{err}");
    }

    #[test]
    fn a_missing_path_says_so_instead_of_panicking() {
        let err = resolve("/definitely/not/here/at/all").expect_err("must fail");
        assert!(err.contains("no encuentro"), "{err}");
    }
}
