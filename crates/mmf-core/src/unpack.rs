use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Result of unpacking a single archive.
#[derive(Debug, Clone)]
pub struct UnpackReport {
    pub source: PathBuf,
    pub dest: PathBuf,
    pub files_written: usize,
    /// Nested archives discovered inside the extracted tree (not yet recursed).
    pub nested_archives: Vec<PathBuf>,
}

/// Extract a `.zip` archive into `dest_root/<archive-stem>/`.
///
/// Returns a report including any nested archives found in the output, which
/// callers may choose to unpack in turn. Guards against zip-slip path escapes.
pub fn unpack_zip(archive: &Path, dest_root: &Path) -> Result<UnpackReport> {
    // A split set is `name.zip.001`, `name.zip.002`, … `resolve_volumes` returns
    // just `[archive]` for a plain single `.zip`.
    let volumes = crate::pack::resolve_volumes(archive)?;

    // Strip a `.NNN` split suffix to get the logical archive name, then its stem.
    let fname = archive
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::Unpack(format!("bad archive name: {}", archive.display())))?;
    let logical = strip_split_suffix(fname);
    let stem = Path::new(logical)
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::Unpack(format!("bad archive name: {}", archive.display())))?;
    let dest = dest_root.join(stem);

    if volumes.len() == 1 {
        return unpack_zip_into(&volumes[0], &dest);
    }

    // The zip reader needs a single seekable file, so concatenate the volumes
    // into a temp `.zip` first, then extract that.
    let tmp = std::env::temp_dir().join(format!(
        "minihoard-zipjoin-{}-{stem}.zip",
        std::process::id()
    ));
    {
        let mut reader = crate::pack::MultiFileReader::open(volumes)?;
        let mut out = std::fs::File::create(&tmp)?;
        std::io::copy(&mut reader, &mut out)?;
    }
    let result = unpack_zip_into(&tmp, &dest);
    let _ = std::fs::remove_file(&tmp);
    let mut report = result?;
    report.source = archive.to_path_buf();
    Ok(report)
}

/// Strip a trailing `.NNN` split-volume suffix (`name.zip.001` → `name.zip`);
/// returns the name unchanged when it isn't a split volume.
fn strip_split_suffix(name: &str) -> &str {
    let is_split = name
        .rsplit('.')
        .next()
        .is_some_and(|s| s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit()));
    if is_split {
        &name[..name.rfind('.').unwrap()]
    } else {
        name
    }
}

/// Extract a `.zip` directly into `dest` (no stem subfolder). Multiple archives
/// can be extracted into the same `dest` to merge them.
pub fn unpack_zip_into(archive: &Path, dest: &Path) -> Result<UnpackReport> {
    std::fs::create_dir_all(dest)?;

    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| Error::Unpack(format!("{}: {e}", archive.display())))?;

    let mut files_written = 0;
    let mut nested_archives = Vec::new();

    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| Error::Unpack(format!("entry {i}: {e}")))?;

        // Use the sanitized name to defend against zip-slip (`../` escapes).
        let Some(rel) = entry.enclosed_name() else {
            return Err(Error::Unpack(format!(
                "unsafe path in archive: {}",
                entry.name()
            )));
        };
        let out_path = dest.join(rel);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&out_path)?;
        std::io::copy(&mut entry, &mut out)?;
        files_written += 1;

        if is_archive(&out_path) {
            nested_archives.push(out_path);
        }
    }

    Ok(UnpackReport {
        source: archive.to_path_buf(),
        dest: dest.to_path_buf(),
        files_written,
        nested_archives,
    })
}

/// True if the path looks like a supported archive by extension.
pub fn is_archive(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("zip")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = Default::default();
        for (name, data) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn extracts_files_into_stem_dir() {
        let dir = std::env::temp_dir().join("minihoard-unpack-test");
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("release.zip");
        make_zip(&archive, &[("a.stl", b"solid a"), ("sub/b.stl", b"solid b")]);

        let report = unpack_zip(&archive, &dir).unwrap();
        assert_eq!(report.files_written, 2);
        assert!(dir.join("release/a.stl").exists());
        assert!(dir.join("release/sub/b.stl").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detects_nested_archives() {
        let dir = std::env::temp_dir().join("minihoard-unpack-nested");
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("outer.zip");
        make_zip(&archive, &[("inner.zip", b"PK\x03\x04not-real")]);

        let report = unpack_zip(&archive, &dir).unwrap();
        assert_eq!(report.nested_archives.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
