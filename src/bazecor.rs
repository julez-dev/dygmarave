//! Detects whether Bazecor is running.
//!
//! Bazecor speaks the same Focus protocol over the same serial port. Neither it
//! nor this daemon requests an exclusive lock, so the OS will not refuse the
//! second opener; the only safe policy is to stop writing while Bazecor runs.
//! Detection is a process-table scan because there is no lock to consult.
//!
//! A scan is a handful of file reads once a second. That is cheap next to the
//! damage two writers interleaving command fragments would do to the board.

use std::path::Path;

/// The name Bazecor's process carries. Its Electron binary is named `bazecor`.
const PROCESS: &str = "bazecor";

/// True when Bazecor is running.
pub fn running() -> bool {
    running_in(Path::new("/proc"))
}

/// True when a process named like Bazecor appears in a `/proc`-like directory.
///
/// Takes the root as a parameter so tests can point at a temporary tree instead
/// of the real process table. A missing or unreadable root means "not running":
/// the daemon must not refuse to work just because it cannot look.
pub fn running_in(proc_root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        // Only numeric names are processes; `comm` holds the short name.
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            return false;
        }
        std::fs::read_to_string(entry.path().join("comm"))
            .map(|name| name.to_ascii_lowercase().contains(PROCESS))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    /// A throwaway `/proc`-like tree whose numbered entries carry `comm` files.
    fn fake_proc(names: &[&str]) -> PathBuf {
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("dygmarave-bazecor-{}-{unique}", std::process::id()));
        for (index, name) in names.iter().enumerate() {
            let dir = root.join((1000 + index).to_string());
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("comm"), format!("{name}\n")).unwrap();
        }
        root
    }

    #[test]
    fn a_bazecor_process_is_detected() {
        let root = fake_proc(&["bazecor"]);
        assert!(running_in(&root));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn detection_ignores_case() {
        let root = fake_proc(&["Bazecor"]);
        assert!(running_in(&root));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_unrelated_process_table_is_not_bazecor() {
        let root = fake_proc(&["firefox", "electron", "bash"]);
        assert!(!running_in(&root));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_missing_process_table_is_not_bazecor() {
        // The daemon must not refuse to work just because it cannot look.
        assert!(!running_in(Path::new("/nonexistent-dygmarave-proc")));
    }

    #[test]
    fn non_numeric_entries_are_ignored() {
        let root = fake_proc(&["firefox"]);
        fs::write(root.join("meminfo"), "not a process\n").unwrap();
        assert!(!running_in(&root));
        fs::remove_dir_all(root).ok();
    }
}
