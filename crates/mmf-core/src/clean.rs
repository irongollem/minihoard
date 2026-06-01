//! Filesystem tidying for downloaded releases: stl-pack-style name cleaning and
//! removal of macOS archive cruft.

use std::path::Path;

use walkdir::WalkDir;

/// Normalize a name for use as a path component: trim, lowercase, and replace
/// spaces and path separators with underscores. Mirrors stl-pack's convention.
pub fn clean_name(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            ' ' | '\t' | '/' | '\\' => out.push('_'),
            // Drop characters that are awkward in filenames across platforms.
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => {}
            c => out.push(c),
        }
    }
    // Collapse repeated underscores and trim them off the ends.
    let collapsed = out
        .split('_')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if collapsed.is_empty() {
        "untitled".into()
    } else {
        collapsed
    }
}

/// Remove macOS artifacts from a tree: `__MACOSX/` directories, `.DS_Store`
/// files, and `._*` AppleDouble files. Returns how many entries were removed.
pub fn strip_apple_artifacts(root: &Path) -> usize {
    let mut removed = 0;

    // Collect first (don't mutate while walking).
    let mut dirs_to_remove = Vec::new();
    let mut files_to_remove = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy();
        if entry.file_type().is_dir() {
            if name == "__MACOSX" {
                dirs_to_remove.push(entry.path().to_path_buf());
            }
        } else if name == ".DS_Store" || name.starts_with("._") {
            files_to_remove.push(entry.path().to_path_buf());
        }
    }
    for f in files_to_remove {
        if std::fs::remove_file(&f).is_ok() {
            removed += 1;
        }
    }
    for d in dirs_to_remove {
        if std::fs::remove_dir_all(&d).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_names() {
        assert_eq!(clean_name("  Parasite Collectibles "), "parasite_collectibles");
        assert_eq!(clean_name("Belkey, The Knellmaster - 3 SCALES"), "belkey,_the_knellmaster_-_3_scales");
        assert_eq!(clean_name("a / b : c"), "a_b_c");
        assert_eq!(clean_name(""), "untitled");
    }

    #[test]
    fn strips_apple_cruft() {
        let dir = std::env::temp_dir().join("minihoard-clean-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("__MACOSX")).unwrap();
        std::fs::write(dir.join("__MACOSX/x"), b"j").unwrap();
        std::fs::write(dir.join(".DS_Store"), b"j").unwrap();
        std::fs::write(dir.join("._model.stl"), b"j").unwrap();
        std::fs::write(dir.join("model.stl"), b"solid").unwrap();

        let n = strip_apple_artifacts(&dir);
        assert!(n >= 2);
        assert!(dir.join("model.stl").exists());
        assert!(!dir.join(".DS_Store").exists());
        assert!(!dir.join("._model.stl").exists());
        assert!(!dir.join("__MACOSX").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
