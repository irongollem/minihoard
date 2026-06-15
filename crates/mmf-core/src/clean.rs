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

/// Normalize a folder name for redundant-wrapper comparison: lowercase and keep
/// only alphanumerics, so separators, spaces, and punctuation are ignored
/// (`Foo Bar`, `foo_bar`, `FOO-BAR` all share a key).
fn norm_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Collapse a redundant *doubled* folder left by archives that wrap their
/// contents in a folder of the same name (giving `Release/Release/files`). Only
/// a lone subdirectory whose name matches the parent's — case-insensitively,
/// ignoring separators and punctuation — is treated as redundant: its contents
/// are moved up, the empty wrapper removed, and `dir` renamed to the inner
/// folder's [`clean_name`]. A differently-named single child (e.g.
/// `Foo/Supported`) is real structure and is left untouched. Repeats while the
/// same-name doubling continues.
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
        // Only collapse a TRUE redundant wrapper: the lone child must have the
        // same name as its parent (case-insensitive, ignoring separators and
        // punctuation), e.g. `Foo/Foo`. A differently-named single child —
        // `Foo/Supported`, `Foo/Bar` — is real structure and is left untouched.
        let parent_name = current
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if norm_key(&child.file_name().to_string_lossy()) != norm_key(&parent_name) {
            break;
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

/// Tidy one release folder in place: strip macOS artifacts, then collapse any
/// redundant single-folder nesting. Returns the folder's final path (it may have
/// been renamed by the flatten).
pub fn tidy_dir(dir: &Path) -> std::io::Result<PathBuf> {
    strip_apple_artifacts(dir);
    flatten_single_dir(dir)
}

/// True if a folder name looks like a `{creator}-{MM-YYYY}` (or `{creator}-undated`)
/// month group produced by the download pipeline. Used to avoid mistaking a group
/// for a release folder when tidying a whole library.
pub fn looks_like_month_group(name: &str) -> bool {
    if let Some(stripped) = name.strip_suffix("-undated") {
        return !stripped.is_empty();
    }
    let b = name.as_bytes();
    let n = b.len();
    // Trailing "-MM-YYYY" (8 chars), with a non-empty creator before it.
    n >= 9
        && b[n - 8] == b'-'
        && b[n - 7].is_ascii_digit()
        && b[n - 6].is_ascii_digit()
        && b[n - 5] == b'-'
        && b[n - 4].is_ascii_digit()
        && b[n - 3].is_ascii_digit()
        && b[n - 2].is_ascii_digit()
        && b[n - 1].is_ascii_digit()
}

/// Enumerate the release folders under a library root: the children of each
/// `{creator}-{MM-YYYY}` month group, plus any release folder sitting directly
/// under the root. Month-group folders themselves are never returned (so tidying
/// never collapses the group/month structure).
pub fn library_release_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for e in entries.filter_map(|e| e.ok()) {
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if looks_like_month_group(&name) {
            // A group → its children are the releases.
            if let Ok(subs) = std::fs::read_dir(e.path()) {
                for s in subs.filter_map(|s| s.ok()) {
                    if s.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        out.push(s.path());
                    }
                }
            }
        } else {
            // A release folder sitting directly under the root.
            out.push(e.path());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_month_groups() {
        assert!(looks_like_month_group("dungeon_classics-05-2026"));
        assert!(looks_like_month_group("one_page_rules-undated"));
        assert!(!looks_like_month_group("Behir"));
        assert!(!looks_like_month_group("Knellkins"));
        assert!(!looks_like_month_group("-05-2026")); // empty creator
    }

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
    fn flattens_exact_name_doubling() {
        let base = tmp("minihoard-flat-1");
        let dir = base.join("behir");
        std::fs::create_dir_all(dir.join("Behir/Supported")).unwrap();
        std::fs::write(dir.join("Behir/thumb.jpg"), b"x").unwrap();
        std::fs::write(dir.join("Behir/Supported/a.stl"), b"x").unwrap();
        std::fs::write(dir.join(".DS_Store"), b"junk").unwrap(); // ignored

        let out = flatten_single_dir(&dir).unwrap();
        // `behir/Behir` is a same-name double (case-insensitive) → collapse.
        assert_eq!(out, base.join("behir"));
        assert!(out.join("Supported/a.stl").exists());
        assert!(out.join("thumb.jpg").exists());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn collapses_doubling_but_keeps_supported_unsupported_siblings() {
        // A same-name double around a folder that itself has multiple children:
        // collapse the doubling, but never merge or descend into the variant
        // folders — they stay distinct siblings under the model.
        let base = tmp("minihoard-flat-siblings");
        let dir = base.join("behir");
        std::fs::create_dir_all(dir.join("Behir/Supported")).unwrap();
        std::fs::create_dir_all(dir.join("Behir/Unsupported")).unwrap();
        std::fs::write(dir.join("Behir/Supported/s.stl"), b"x").unwrap();
        std::fs::write(dir.join("Behir/Unsupported/u.stl"), b"x").unwrap();
        std::fs::write(dir.join("Behir/thumb.jpg"), b"x").unwrap();

        let out = flatten_single_dir(&dir).unwrap();
        assert_eq!(out, base.join("behir"));
        assert!(out.join("Supported/s.stl").exists());
        assert!(out.join("Unsupported/u.stl").exists());
        assert!(out.join("thumb.jpg").exists());
        assert!(out.join("Supported").is_dir());
        assert!(out.join("Unsupported").is_dir());
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
    fn does_not_collapse_differently_named_or_variant_child() {
        let base = tmp("minihoard-flat-3");
        // A differently-named single wrapper is real structure → left as-is.
        let dir = base.join("rel");
        std::fs::create_dir_all(dir.join("Inner")).unwrap();
        std::fs::write(dir.join("Inner/model.stl"), b"x").unwrap();
        let out = flatten_single_dir(&dir).unwrap();
        assert_eq!(out, dir); // "rel" != "Inner"
        assert!(out.join("Inner/model.stl").exists());

        // A model that ships only `Supported/` keeps its own name.
        let m = base.join("mummies");
        std::fs::create_dir_all(m.join("Supported")).unwrap();
        std::fs::write(m.join("Supported/s.stl"), b"x").unwrap();
        let out2 = flatten_single_dir(&m).unwrap();
        assert_eq!(out2, m); // not renamed to "supported"
        assert!(out2.join("Supported/s.stl").exists());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn collapses_repeated_same_name_doubling() {
        let base = tmp("minihoard-flat-4");
        let dir = base.join("foo");
        std::fs::create_dir_all(dir.join("Foo/foo")).unwrap();
        std::fs::write(dir.join("Foo/foo/model.stl"), b"x").unwrap();

        let out = flatten_single_dir(&dir).unwrap();
        assert_eq!(out, base.join("foo"));
        assert!(out.join("model.stl").exists());
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
