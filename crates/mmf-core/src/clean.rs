//! Filesystem tidying for downloaded releases: stl-pack-style name cleaning and
//! removal of macOS archive cruft.

use std::path::{Path, PathBuf};

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

/// True if a path component is a macOS filesystem artifact that should never be
/// archived: `.DS_Store`, `._*` AppleDouble files, or a `__MACOSX` directory.
/// macOS regenerates `.DS_Store` whenever a folder is viewed in Finder, so packers
/// must filter on this directly rather than trusting an earlier strip to have stuck.
pub fn is_apple_artifact(name: &str) -> bool {
    name == ".DS_Store" || name == "__MACOSX" || name.starts_with("._")
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
        if !is_apple_artifact(&name) {
            continue;
        }
        if entry.file_type().is_dir() {
            dirs_to_remove.push(entry.path().to_path_buf());
        } else {
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

/// Collapse redundant single-folder nesting left by archives that contain one
/// top-level folder (giving `Release/Release/files`). If `dir` holds nothing but
/// a single subdirectory — macOS artifacts ignored — that wrapper is pointless:
/// the inner folder's contents are moved up, the empty inner folder is removed,
/// and `dir` is renamed to the inner folder's [`clean_name`]. Repeats for
/// multiple nested levels (`A/B/C/files` → `c/files`).
///
/// Returns the directory's final path (it may have been renamed). If the target
/// name already exists as a sibling, the contents are still collapsed but the
/// original name is kept (no clobbering).
pub fn flatten_single_dir(dir: &Path) -> std::io::Result<PathBuf> {
    let mut current = dir.to_path_buf();
    loop {
        // The lone non-artifact child, if that's all `current` holds.
        let mut kids: Vec<_> = std::fs::read_dir(&current)?
            .filter_map(|e| e.ok())
            .filter(|e| !is_apple_artifact(&e.file_name().to_string_lossy()))
            .collect();
        if kids.len() != 1 {
            break;
        }
        let child = kids.pop().unwrap();
        if !child.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            break; // single entry, but it's a file — nothing to collapse
        }
        let child_path = child.path();

        // Move the child's real contents up (skip artifacts — they go away with
        // the child). `current` held only the child, so no real-file collisions.
        let inner: Vec<_> = std::fs::read_dir(&child_path)?
            .filter_map(|e| e.ok())
            .filter(|e| !is_apple_artifact(&e.file_name().to_string_lossy()))
            .collect();
        for entry in inner {
            std::fs::rename(entry.path(), current.join(entry.file_name()))?;
        }
        std::fs::remove_dir_all(&child_path)?;

        // Rename `current` to the inner folder's cleaned name, unless that would
        // collide with an existing sibling.
        let new_name = clean_name(&child.file_name().to_string_lossy());
        if let Some(parent) = current.parent() {
            let candidate = parent.join(&new_name);
            if candidate != current && !candidate.exists() {
                std::fs::rename(&current, &candidate)?;
                current = candidate;
            }
        }
    }
    Ok(current)
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

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn flattens_single_wrapper_and_adopts_inner_name() {
        let base = tmp("minihoard-flat-1");
        let dir = base.join("behir_presupported");
        std::fs::create_dir_all(dir.join("Behir/Supported")).unwrap();
        std::fs::write(dir.join("Behir/thumb.jpg"), b"x").unwrap();
        std::fs::write(dir.join("Behir/Supported/a.stl"), b"x").unwrap();
        std::fs::write(dir.join(".DS_Store"), b"junk").unwrap(); // ignored

        let out = flatten_single_dir(&dir).unwrap();
        assert_eq!(out, base.join("behir")); // renamed to inner (cleaned)
        assert!(out.join("Supported/a.stl").exists());
        assert!(out.join("thumb.jpg").exists());
        assert!(!base.join("behir_presupported").exists());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn does_not_flatten_when_other_content_present() {
        let base = tmp("minihoard-flat-2");
        let dir = base.join("rel");
        std::fs::create_dir_all(dir.join("Inner")).unwrap();
        std::fs::write(dir.join("readme.txt"), b"x").unwrap(); // extra file → keep

        let out = flatten_single_dir(&dir).unwrap();
        assert_eq!(out, dir);
        assert!(dir.join("Inner").exists());
        assert!(dir.join("readme.txt").exists());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn flattens_multiple_nested_levels() {
        let base = tmp("minihoard-flat-3");
        let dir = base.join("rel");
        std::fs::create_dir_all(dir.join("A/B")).unwrap();
        std::fs::write(dir.join("A/B/model.stl"), b"x").unwrap();

        let out = flatten_single_dir(&dir).unwrap();
        assert_eq!(out, base.join("b"));
        assert!(out.join("model.stl").exists());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn keeps_name_on_collision_but_still_collapses() {
        let base = tmp("minihoard-flat-4");
        std::fs::create_dir_all(base.join("inner")).unwrap(); // occupies target name
        let dir = base.join("wrapper");
        std::fs::create_dir_all(dir.join("Inner")).unwrap();
        std::fs::write(dir.join("Inner/a.stl"), b"x").unwrap();

        let out = flatten_single_dir(&dir).unwrap();
        assert_eq!(out, dir); // not renamed (would collide with base/inner)
        assert!(dir.join("a.stl").exists()); // but still collapsed
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn leaves_plain_file_layout_alone() {
        let base = tmp("minihoard-flat-5");
        let dir = base.join("rel");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("only.stl"), b"x").unwrap(); // single FILE, not dir

        let out = flatten_single_dir(&dir).unwrap();
        assert_eq!(out, dir);
        assert!(dir.join("only.stl").exists());
        std::fs::remove_dir_all(&base).ok();
    }
}
