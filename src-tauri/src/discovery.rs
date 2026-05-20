use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ignore::{WalkBuilder, WalkState};

const HARD_EXCLUDE: &[&str] = &[
    // Build/tool caches that never legitimately contain user repos and
    // routinely contain thousands of files.
    "node_modules",
    "target",
    "dist",
    ".cache",
    "vendor",
    ".git",
    // Privacy-sensitive: credentials, keys, kube/cloud configs. gitwink
    // only looks for `.git` entries — but walking into these dirs at all
    // is at odds with the "local-only, nothing leaves your machine" promise.
    ".ssh",
    ".aws",
    ".azure",
    ".gnupg",
    ".gpg",
    ".kube",
    ".docker",
    // OS-managed user/system data trees. Reachable on non-system drives
    // (e.g. cloned profile under D:\Users\...) or if a user keeps repos
    // under Documents/AppData/Library — never useful, often huge.
    "AppData",
    "ProgramData",
    "Library",
];

const MAX_DEPTH: usize = 8;

/// Whether a directory name is in the hard-exclude set. Exposed so the
/// explicit-add nested-repo discovery can skip the same names when
/// walking a super-repo's children.
pub fn is_hard_excluded(name: &str) -> bool {
    HARD_EXCLUDE
        .iter()
        .any(|excl| name.eq_ignore_ascii_case(excl))
}

pub fn default_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    #[cfg(windows)]
    {
        if let Ok(profile) = std::env::var("USERPROFILE") {
            let base = PathBuf::from(profile);
            for sub in [
                "source",
                "Documents",
                "Projects",
                "Code",
                "Dev",
                "repos",
                "Desktop",
            ] {
                roots.push(base.join(sub));
            }
        }

        // Skip the system drive to avoid scanning C:\Windows, Program Files,
        // ProgramData, etc. — for that drive we rely on the USERPROFILE-based
        // dirs above. Every other attached drive (A, D, E, ...) gets walked
        // from its root with depth + hard-exclude doing the cutoff work.
        let system_drive = std::env::var("SystemDrive")
            .unwrap_or_else(|_| "C:".into())
            .to_uppercase();
        let system_letter = system_drive.chars().next().unwrap_or('C');

        for letter in b'A'..=b'Z' {
            let ch = letter as char;
            if ch == system_letter {
                continue;
            }
            let root = PathBuf::from(format!("{ch}:\\"));
            if root.is_dir() {
                roots.push(root);
            }
        }
    }

    #[cfg(not(windows))]
    {
        if let Ok(home) = std::env::var("HOME") {
            let base = PathBuf::from(home);
            for sub in ["Projects", "Code", "Documents", "Developer"] {
                roots.push(base.join(sub));
            }
        }
    }

    // Dedupe and filter to existing dirs.
    let mut seen = std::collections::HashSet::new();
    roots
        .into_iter()
        .filter(|p| p.is_dir())
        .filter(|p| {
            let key = p
                .canonicalize()
                .unwrap_or_else(|_| p.clone())
                .to_string_lossy()
                .into_owned();
            seen.insert(key)
        })
        .collect()
}

/// Walk `root` and call `on_repo(path)` for every directory containing a
/// `.git` entry. Descent into a discovered repo is skipped, as are
/// `HARD_EXCLUDE` directories. Errors during walk are swallowed.
pub fn scan_path<F>(root: &Path, on_repo: F)
where
    F: FnMut(PathBuf) + Send,
{
    let on_repo = Mutex::new(on_repo);

    WalkBuilder::new(root)
        .max_depth(Some(MAX_DEPTH))
        .standard_filters(false)
        .hidden(false)
        .filter_entry(|entry| {
            !is_hard_excluded(&entry.file_name().to_string_lossy())
        })
        .build_parallel()
        .run(|| {
            Box::new(|result| {
                let entry = match result {
                    Ok(e) => e,
                    Err(_) => return WalkState::Continue,
                };
                let is_dir = entry
                    .file_type()
                    .map(|t| t.is_dir())
                    .unwrap_or(false);
                if !is_dir {
                    return WalkState::Continue;
                }
                let path = entry.path();
                if path.join(".git").exists() {
                    if let Ok(mut cb) = on_repo.lock() {
                        cb(path.to_path_buf());
                    }
                    return WalkState::Skip;
                }
                WalkState::Continue
            })
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_repo(root: &Path, sub: &str) {
        let p = root.join(sub);
        fs::create_dir_all(&p).unwrap();
        fs::create_dir_all(p.join(".git")).unwrap();
    }

    fn collect(root: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        scan_path(root, |p| found.push(p));
        found.sort();
        found
    }

    #[test]
    fn finds_top_level_repo() {
        let dir = TempDir::new().unwrap();
        make_repo(dir.path(), "alpha");
        let found = collect(dir.path());
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("alpha"));
    }

    #[test]
    fn finds_nested_repo() {
        let dir = TempDir::new().unwrap();
        make_repo(dir.path(), "workspace/proj-a");
        make_repo(dir.path(), "workspace/proj-b");
        let found = collect(dir.path());
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn stops_descending_at_git() {
        let dir = TempDir::new().unwrap();
        make_repo(dir.path(), "outer");
        // Nested fake submodule with its own .git — must NOT be reported
        // because we stop descending into `outer` once we see its .git.
        make_repo(dir.path(), "outer/inner");
        let found = collect(dir.path());
        assert_eq!(found.len(), 1, "expected only outer, got {:?}", found);
        assert!(found[0].ends_with("outer"));
    }

    #[test]
    fn skips_hard_excluded_dirs() {
        let dir = TempDir::new().unwrap();
        make_repo(dir.path(), "node_modules/some-pkg");
        make_repo(dir.path(), "target/inner");
        make_repo(dir.path(), "vendor/x");
        make_repo(dir.path(), ".cache/y");
        make_repo(dir.path(), "real-repo");
        let found = collect(dir.path());
        assert_eq!(found.len(), 1, "got {:?}", found);
        assert!(found[0].ends_with("real-repo"));
    }

    #[test]
    fn skips_privacy_sensitive_dirs() {
        let dir = TempDir::new().unwrap();
        // A repo cloned into a sensitive dir must NOT be reported.
        make_repo(dir.path(), ".ssh/repo-in-ssh");
        make_repo(dir.path(), ".aws/repo-in-aws");
        make_repo(dir.path(), "AppData/Local/repo-in-appdata");
        make_repo(dir.path(), "Library/repo-in-library");
        // Control case at the top level should still come through.
        make_repo(dir.path(), "real-repo");

        let found = collect(dir.path());
        assert_eq!(found.len(), 1, "got {:?}", found);
        assert!(found[0].ends_with("real-repo"));
    }

    #[test]
    fn respects_max_depth() {
        let dir = TempDir::new().unwrap();
        // 10 levels deep — must NOT be found at depth cap 8.
        let mut nested = PathBuf::from("a/b/c/d/e/f/g/h/i/j");
        make_repo(dir.path(), nested.to_str().unwrap());
        // Shallow repo at depth 1 — must be found.
        make_repo(dir.path(), "shallow");
        let found = collect(dir.path());
        nested = dir.path().join(nested);
        assert!(found.iter().any(|p| p.ends_with("shallow")));
        assert!(
            !found.iter().any(|p| *p == nested),
            "deep repo unexpectedly found at depth > MAX_DEPTH"
        );
    }
}
