//! Where the app keeps its own files.

use std::path::PathBuf;

use directories::ProjectDirs;

/// The directory the app owns: conversation, memory and the audit log.
///
/// `NEREA_DATA_DIR` points it somewhere else, which is how tests and a
/// throwaway session keep their state apart.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NEREA_DATA_DIR") {
        return PathBuf::from(dir);
    }

    ProjectDirs::from("", "", "nerea")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_dir_is_absolute_or_the_current_one() {
        let dir = data_dir();
        assert!(dir.is_absolute() || dir.as_os_str() == ".");
    }
}
