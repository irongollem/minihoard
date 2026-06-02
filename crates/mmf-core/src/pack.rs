//! Repack a clean release folder into a single archive for off-site backup.
//!
//! Two formats, two jobs:
//! - [`PackFormat::TarZst`] — `tar` streamed through `zstd`. Best ratio + speed,
//!   and the only format that supports splitting into fixed-size volumes (for
//!   2/4 GB backup chunks). No random access: reading one file means streaming
//!   the whole archive. This is the archival default — it replaces hand-driven
//!   7-Zip.
//! - [`PackFormat::Zip`] — Deflate `.zip`. Broadly supported, native
//!   double-click extraction, and random-access (a catalog can read one entry
//!   without unpacking everything). Single file only — no splitting.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::clean::is_apple_artifact;
use crate::error::{Error, Result};

/// Output archive format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackFormat {
    /// `tar` + `zstd` (`.tar.zst`). Best compression; supports `--split`.
    TarZst,
    /// Deflate `.zip`. Broadly supported, random-access; single file only.
    Zip,
}

impl PackFormat {
    /// The filename extension (without leading dot) for this format.
    pub fn ext(self) -> &'static str {
        match self {
            PackFormat::TarZst => "tar.zst",
            PackFormat::Zip => "zip",
        }
    }

    /// Parse a `--format` value (`tarzst`/`tar.zst`/`zst`/`zstd` or `zip`).
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "tarzst" | "tar.zst" | "tar+zst" | "zst" | "zstd" => Ok(PackFormat::TarZst),
            "zip" => Ok(PackFormat::Zip),
            other => Err(Error::Unpack(format!(
                "unknown pack format `{other}` (expected `tarzst` or `zip`)"
            ))),
        }
    }
}

/// How to pack.
pub struct PackOptions {
    pub format: PackFormat,
    /// zstd compression level (1–22). Ignored for zip. ~19 is a good archival
    /// default; higher is slower for marginal gains on mesh data.
    pub level: i32,
    /// Roll output into volumes of this many bytes each (tar.zst only).
    pub split_bytes: Option<u64>,
    /// Write a `<archive>.json` sidecar listing the contents (default: yes).
    /// Lets a catalog see what's inside — especially for tar.zst, which has no
    /// random access — without decompressing the archive.
    pub write_sidecar: bool,
}

impl Default for PackOptions {
    fn default() -> Self {
        PackOptions {
            format: PackFormat::TarZst,
            level: 19,
            split_bytes: None,
            write_sidecar: true,
        }
    }
}

/// What was produced.
pub struct PackReport {
    /// Archive files written, in order (one, or many when split).
    pub outputs: Vec<PathBuf>,
    /// The `<archive>.json` content index, if written.
    pub sidecar: Option<PathBuf>,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub file_count: usize,
}

/// A `.json` index written beside an archive so its contents are knowable
/// without decompressing it. Schema is versioned via [`Sidecar::schema`].
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Sidecar {
    /// Schema tag, e.g. `minihoard-pack/1`.
    pub schema: String,
    /// The packed folder's name (the release).
    pub name: String,
    /// Archive format: `tar.zst` or `zip`.
    pub format: String,
    /// Unix epoch seconds when packed.
    pub created_unix: u64,
    /// Creator, parsed from the parent `{creator}-{MM-YYYY}` folder if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    /// Release month `MM-YYYY`, parsed from the parent folder if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_month: Option<String>,
    pub file_count: usize,
    pub uncompressed_bytes: u64,
    pub compressed_bytes: u64,
    /// Archive volume filenames in order (one, or many when split).
    pub volumes: Vec<String>,
    /// Every file in the archive, categorized for search/preview.
    pub entries: Vec<Entry>,
}

/// One file inside a packed archive.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    /// Path inside the archive (e.g. `DragonKnight/parts/arm.stl`).
    pub path: String,
    pub bytes: u64,
    /// `model` | `image` | `doc` | `other` — lets a catalog find printable
    /// meshes and preview images without opening the archive.
    pub kind: String,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Walk `src` skipping macOS artifacts (`.DS_Store`, `._*`, `__MACOSX/`). Used by
/// every pack-time walk so the measured size, the archive, and the sidecar index
/// all agree and never carry Finder cruft, regardless of what's on disk. Pruning
/// happens via `filter_entry` so `__MACOSX` directories aren't descended into.
fn walk_packable(src: &Path) -> impl Iterator<Item = walkdir::DirEntry> {
    WalkDir::new(src)
        .into_iter()
        .filter_entry(|e| !is_apple_artifact(&e.file_name().to_string_lossy()))
        .filter_map(|e| e.ok())
}

/// Categorize a file by extension for the sidecar index.
fn classify(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "stl" | "obj" | "3mf" | "ply" | "step" | "stp" | "fbx" | "lys" | "ctb" | "blend" => "model",
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" => "image",
        "txt" | "pdf" | "md" | "rtf" | "doc" | "docx" | "nfo" => "doc",
        _ => "other",
    }
}

/// Collect every file under `src` as sidecar [`Entry`]s, paths relative to
/// `src`'s parent (so the top folder is included, matching the archive layout).
fn collect_entries(src: &Path) -> Vec<Entry> {
    let parent = src.parent().unwrap_or(src);
    walk_packable(src)
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            let path = e.path();
            let rel = path.strip_prefix(parent).unwrap_or(path);
            Entry {
                path: rel.to_string_lossy().replace('\\', "/"),
                bytes: e.metadata().map(|m| m.len()).unwrap_or(0),
                kind: classify(path).to_string(),
            }
        })
        .collect()
}

/// Parse a `{creator}-{MM-YYYY}` group folder name into `(creator, MM-YYYY)`.
fn parse_group(parent: &Path) -> (Option<String>, Option<String>) {
    let Some(name) = parent.file_name().and_then(|s| s.to_str()) else {
        return (None, None);
    };
    // Expect a trailing `-MM-YYYY`.
    let bytes = name.as_bytes();
    if name.len() >= 8 {
        let tail = &name[name.len() - 7..]; // `MM-YYYY`
        let tb = tail.as_bytes();
        let shaped = tb[2] == b'-'
            && tb[..2].iter().all(|c| c.is_ascii_digit())
            && tb[3..].iter().all(|c| c.is_ascii_digit());
        if shaped && bytes[name.len() - 8] == b'-' {
            let creator = &name[..name.len() - 8];
            if !creator.is_empty() {
                return (Some(creator.to_string()), Some(tail.to_string()));
            }
        }
    }
    (None, None)
}

/// Parse a human size like `2G`, `512M`, `4GB`, `1500000`. Binary units (×1024).
pub fn parse_size(s: &str) -> Result<u64> {
    let t = s.trim();
    let digits_end = t
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(t.len());
    let (num, unit) = t.split_at(digits_end);
    let num: f64 = num
        .parse()
        .map_err(|_| Error::Unpack(format!("invalid size `{s}`")))?;
    let mult: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        other => return Err(Error::Unpack(format!("unknown size unit `{other}` in `{s}`"))),
    };
    let bytes = (num * mult) as u64;
    if bytes == 0 {
        return Err(Error::Unpack(format!("size must be > 0 (`{s}`)")));
    }
    Ok(bytes)
}

/// Sum file sizes and count under `src`.
fn measure(src: &Path) -> Result<(u64, usize)> {
    let mut bytes = 0u64;
    let mut count = 0usize;
    for entry in walk_packable(src) {
        if entry.file_type().is_file() {
            bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            count += 1;
        }
    }
    Ok((bytes, count))
}

/// Pack directory `src` into `out_dir/<name>.<ext>` (or `.001`, `.002`, … when
/// split). `name_override` sets the archive's base filename verbatim (for a
/// strict archive naming convention); when `None`, the source folder name is
/// used. The archive's *internal* layout always keeps the real folder name.
/// `on_progress(bytes_read)` reports input bytes consumed.
pub fn pack_dir(
    src: &Path,
    out_dir: &Path,
    opts: &PackOptions,
    name_override: Option<&str>,
    on_progress: impl FnMut(u64),
) -> Result<PackReport> {
    if !src.is_dir() {
        return Err(Error::Unpack(format!(
            "not a directory: {} (pack operates on a release folder)",
            src.display()
        )));
    }
    let name = match name_override {
        Some(n) if !n.trim().is_empty() => n.trim(),
        _ => src
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Error::Unpack(format!("bad folder name: {}", src.display())))?,
    };
    std::fs::create_dir_all(out_dir)?;

    let (input_bytes, file_count) = measure(src)?;

    let (outputs, output_bytes) = match opts.format {
        PackFormat::TarZst => pack_tar_zst(src, out_dir, name, opts, on_progress)?,
        PackFormat::Zip => {
            if opts.split_bytes.is_some() {
                return Err(Error::Unpack(
                    "splitting is only supported for tar.zst (zip needs a seekable, single file)"
                        .to_string(),
                ));
            }
            pack_zip(src, out_dir, name, on_progress)?
        }
    };

    let sidecar = if opts.write_sidecar {
        Some(write_sidecar(
            src,
            name,
            opts.format,
            &outputs,
            input_bytes,
            output_bytes,
        )?)
    } else {
        None
    };

    Ok(PackReport {
        outputs,
        sidecar,
        input_bytes,
        output_bytes,
        file_count,
    })
}

/// Write `<out_dir>/<name>.<ext>.json` describing the archive's contents.
fn write_sidecar(
    src: &Path,
    name: &str,
    format: PackFormat,
    outputs: &[PathBuf],
    input_bytes: u64,
    output_bytes: u64,
) -> Result<PathBuf> {
    let entries = collect_entries(src);
    let (creator, release_month) = match src.parent() {
        Some(p) => parse_group(p),
        None => (None, None),
    };
    let volumes = outputs
        .iter()
        .filter_map(|p| p.file_name().and_then(|s| s.to_str()).map(String::from))
        .collect();
    let sidecar = Sidecar {
        schema: "minihoard-pack/1".to_string(),
        name: name.to_string(),
        format: format.ext().to_string(),
        created_unix: now_unix(),
        creator,
        release_month,
        file_count: entries.len(),
        uncompressed_bytes: input_bytes,
        compressed_bytes: output_bytes,
        volumes,
        entries,
    };
    // Sidecar is named after the logical archive, not a specific volume:
    // `DragonKnight.tar.zst.json` / `DragonKnight.zip.json`.
    let dir = outputs[0].parent().unwrap_or_else(|| Path::new("."));
    let path = dir.join(format!("{name}.{}.json", format.ext()));
    let json = serde_json::to_string_pretty(&sidecar)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

fn pack_tar_zst(
    src: &Path,
    out_dir: &Path,
    name: &str,
    opts: &PackOptions,
    mut on_progress: impl FnMut(u64),
) -> Result<(Vec<PathBuf>, u64)> {
    let base = format!("{name}.{}", PackFormat::TarZst.ext());
    let writer = SplitWriter::new(out_dir.to_path_buf(), base, opts.split_bytes);

    // tar -> zstd -> split file(s). Walk the tree by hand (instead of
    // `append_dir_all`) so we can report per-file input progress.
    let mut encoder = zstd::Encoder::new(writer, opts.level)
        .map_err(|e| Error::Unpack(format!("zstd init: {e}")))?;
    // Use all cores for large mesh libraries when the build supports it.
    let _ = encoder.multithread(num_threads());

    let parent = src.parent().unwrap_or(src);
    let mut done = 0u64;
    {
        let mut builder = tar::Builder::new(&mut encoder);
        builder.follow_symlinks(false);
        for entry in walk_packable(src) {
            let path = entry.path();
            // Keep the top folder: archive paths are relative to src's parent.
            let rel = path.strip_prefix(parent).unwrap_or(path);
            if entry.file_type().is_dir() {
                if !rel.as_os_str().is_empty() {
                    builder
                        .append_dir(rel, path)
                        .map_err(|e| Error::Unpack(format!("tar dir: {e}")))?;
                }
            } else if entry.file_type().is_file() {
                let mut f = File::open(path)?;
                builder
                    .append_file(rel, &mut f)
                    .map_err(|e| Error::Unpack(format!("tar file: {e}")))?;
                done += entry.metadata().map(|m| m.len()).unwrap_or(0);
                on_progress(done);
            }
        }
        builder
            .finish()
            .map_err(|e| Error::Unpack(format!("tar finish: {e}")))?;
    }
    let writer = encoder
        .finish()
        .map_err(|e| Error::Unpack(format!("zstd finish: {e}")))?;
    writer.into_outputs()
}

fn pack_zip(
    src: &Path,
    out_dir: &Path,
    name: &str,
    mut on_progress: impl FnMut(u64),
) -> Result<(Vec<PathBuf>, u64)> {
    let out_path = out_dir.join(format!("{name}.{}", PackFormat::Zip.ext()));
    let file = File::create(&out_path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    let mut buf = Vec::new();
    let mut done = 0u64;
    for entry in walk_packable(src) {
        let path = entry.path();
        // Archive paths are relative to src's parent so the top folder is kept.
        let rel = path
            .strip_prefix(src.parent().unwrap_or(src))
            .unwrap_or(path);
        let rel_str = rel
            .to_str()
            .ok_or_else(|| Error::Unpack(format!("non-UTF-8 path: {}", path.display())))?
            .replace('\\', "/");
        if entry.file_type().is_dir() {
            if !rel_str.is_empty() {
                zip.add_directory(&rel_str, options)
                    .map_err(|e| Error::Unpack(format!("zip dir: {e}")))?;
            }
        } else if entry.file_type().is_file() {
            zip.start_file(&rel_str, options)
                .map_err(|e| Error::Unpack(format!("zip file: {e}")))?;
            let mut f = File::open(path)?;
            f.read_to_end(&mut buf)?;
            zip.write_all(&buf)?;
            done += buf.len() as u64;
            on_progress(done);
            buf.clear();
        }
    }
    zip.finish()
        .map_err(|e| Error::Unpack(format!("zip finish: {e}")))?;

    let output_bytes = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
    Ok((vec![out_path], output_bytes))
}

/// Best-effort core count for zstd's worker threads (1 if undetectable).
fn num_threads() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
}

/// A `Write` that transparently rolls over to a new file every `limit` bytes,
/// numbering volumes `<base>.001`, `<base>.002`, … When `limit` is `None` it
/// writes a single file named exactly `<base>`.
struct SplitWriter {
    dir: PathBuf,
    base: String,
    limit: Option<u64>,
    index: usize,
    current: Option<File>,
    written_in_current: u64,
    total: u64,
    outputs: Vec<PathBuf>,
}

impl SplitWriter {
    fn new(dir: PathBuf, base: String, limit: Option<u64>) -> Self {
        SplitWriter {
            dir,
            base,
            limit,
            index: 0,
            current: None,
            written_in_current: 0,
            total: 0,
            outputs: Vec::new(),
        }
    }

    fn volume_path(&self, index: usize) -> PathBuf {
        match self.limit {
            Some(_) => self.dir.join(format!("{}.{:03}", self.base, index + 1)),
            None => self.dir.join(&self.base),
        }
    }

    fn open_next(&mut self) -> io::Result<()> {
        let path = self.volume_path(self.index);
        let f = File::create(&path)?;
        self.outputs.push(path);
        self.current = Some(f);
        self.written_in_current = 0;
        self.index += 1;
        Ok(())
    }

    fn into_outputs(self) -> Result<(Vec<PathBuf>, u64)> {
        Ok((self.outputs, self.total))
    }
}

impl Write for SplitWriter {
    fn write(&mut self, mut buf: &[u8]) -> io::Result<usize> {
        let start_len = buf.len();
        while !buf.is_empty() {
            if self.current.is_none() {
                self.open_next()?;
            }
            let take = match self.limit {
                Some(limit) => {
                    let room = limit.saturating_sub(self.written_in_current);
                    if room == 0 {
                        // Current volume full — roll to the next one.
                        self.current = None;
                        continue;
                    }
                    (room as usize).min(buf.len())
                }
                None => buf.len(),
            };
            let n = self.current.as_mut().unwrap().write(&buf[..take])?;
            self.written_in_current += n as u64;
            self.total += n as u64;
            buf = &buf[n..];
        }
        Ok(start_len)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(f) = self.current.as_mut() {
            f.flush()
        } else {
            Ok(())
        }
    }
}

// ---- Restore -------------------------------------------------------------

/// True if `path` looks like a tar.zst archive or its first split volume.
pub fn is_tar_zst(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    name.ends_with(".tar.zst")
        || name.ends_with(".tzst")
        || (name.contains(".tar.zst.") && name.rsplit('.').next().is_some_and(|s| s.parse::<u32>().is_ok()))
}

/// Delete an archive and everything that belongs to it: all split volumes (for
/// a `*.tar.zst.NNN` set) plus the `<archive>.json` sidecar. Use after a
/// verified extraction. Returns the number of files removed. Works for `.zip`
/// and single or split `.tar.zst`.
pub fn remove_archive_files(archive: &Path) -> Result<usize> {
    let mut removed = 0;
    for v in resolve_volumes(archive)? {
        if std::fs::remove_file(&v).is_ok() {
            removed += 1;
        }
    }
    if std::fs::remove_file(sidecar_path_for(archive)).is_ok() {
        removed += 1;
    }
    Ok(removed)
}

/// The sidecar path for an archive: `<logical-archive>.json`. For a split
/// volume `name.tar.zst.001` the logical archive is `name.tar.zst`.
fn sidecar_path_for(archive: &Path) -> PathBuf {
    let dir = archive.parent().unwrap_or_else(|| Path::new("."));
    let name = archive
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let logical = if name.contains(".tar.zst.")
        && name
            .rsplit('.')
            .next()
            .is_some_and(|s| s.len() == 3 && s.chars().all(|c| c.is_ascii_digit()))
    {
        &name[..name.rfind('.').unwrap()]
    } else {
        name
    };
    dir.join(format!("{logical}.json"))
}

/// Extract a tar.zst archive into `dest`. If `archive` is a split volume
/// (`*.tar.zst.001`), all sibling volumes are concatenated in order first.
pub fn unpack_tar_zst(archive: &Path, dest: &Path) -> Result<usize> {
    std::fs::create_dir_all(dest)?;
    let volumes = resolve_volumes(archive)?;
    let reader = MultiFileReader::open(volumes)?;
    let decoder = zstd::Decoder::new(reader).map_err(|e| Error::Unpack(format!("zstd: {e}")))?;
    let mut ar = tar::Archive::new(decoder);

    let mut count = 0usize;
    for entry in ar.entries().map_err(|e| Error::Unpack(format!("tar: {e}")))? {
        let mut entry = entry.map_err(|e| Error::Unpack(format!("tar entry: {e}")))?;
        // tar crate guards against `..` traversal in unpack_in.
        if entry
            .unpack_in(dest)
            .map_err(|e| Error::Unpack(format!("extract: {e}")))?
        {
            if entry.header().entry_type().is_file() {
                count += 1;
            }
        } else {
            return Err(Error::Unpack(format!(
                "unsafe path in archive: {}",
                entry.path().map(|p| p.display().to_string()).unwrap_or_default()
            )));
        }
    }
    Ok(count)
}

/// Given any volume path, return the ordered list of volumes to read. For a
/// single `.tar.zst` that's just `[archive]`; for `name.tar.zst.001` it's all
/// `name.tar.zst.NNN` siblings sorted ascending.
fn resolve_volumes(archive: &Path) -> Result<Vec<PathBuf>> {
    let name = archive
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::Unpack(format!("bad archive name: {}", archive.display())))?;

    // Detect the `.NNN` split suffix.
    let is_split_volume = name.rsplit('.').next().is_some_and(|s| {
        s.len() == 3 && s.chars().all(|c| c.is_ascii_digit())
    }) && name.contains(".tar.zst.");
    if !is_split_volume {
        return Ok(vec![archive.to_path_buf()]);
    }

    let stem = &name[..name.rfind('.').unwrap()]; // strip `.NNN` → `name.tar.zst`
    let dir = archive.parent().unwrap_or_else(|| Path::new("."));
    let mut vols: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|n| {
                    n.starts_with(stem)
                        && n.len() == stem.len() + 4 // ".NNN"
                        && n[stem.len()..].starts_with('.')
                        && n[stem.len() + 1..].chars().all(|c| c.is_ascii_digit())
                })
                .unwrap_or(false)
        })
        .collect();
    vols.sort();
    if vols.is_empty() {
        return Err(Error::Unpack(format!(
            "no split volumes found for {}",
            archive.display()
        )));
    }
    Ok(vols)
}

/// Reads a sequence of files as one continuous byte stream.
struct MultiFileReader {
    files: std::vec::IntoIter<PathBuf>,
    current: Option<File>,
}

impl MultiFileReader {
    fn open(paths: Vec<PathBuf>) -> Result<Self> {
        let mut iter = paths.into_iter();
        let current = match iter.next() {
            Some(p) => Some(File::open(p)?),
            None => None,
        };
        Ok(MultiFileReader {
            files: iter,
            current,
        })
    }
}

impl Read for MultiFileReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.current.as_mut() {
                None => return Ok(0),
                Some(f) => {
                    let n = f.read(buf)?;
                    if n > 0 {
                        return Ok(n);
                    }
                    // Current file exhausted — advance to the next volume.
                    self.current = match self.files.next() {
                        Some(p) => Some(File::open(p)?),
                        None => None,
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tree(root: &Path) {
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.stl"), vec![b'a'; 5000]).unwrap();
        std::fs::write(root.join("sub/b.stl"), vec![b'b'; 5000]).unwrap();
    }

    #[test]
    fn parse_sizes() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("2K").unwrap(), 2048);
        assert_eq!(parse_size("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("2G").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("4gb").unwrap(), 4 * 1024 * 1024 * 1024);
        assert!(parse_size("0").is_err());
        assert!(parse_size("bogus").is_err());
    }

    #[test]
    fn tarzst_round_trip() {
        let tmp = std::env::temp_dir().join("minihoard-pack-tarzst");
        let _ = std::fs::remove_dir_all(&tmp);
        let src = tmp.join("Release-06-2026");
        make_tree(&src);
        let out = tmp.join("out");

        let report = pack_dir(&src, &out, &PackOptions::default(), None, |_| {}).unwrap();
        assert_eq!(report.outputs.len(), 1);
        assert_eq!(report.file_count, 2);
        assert!(report.outputs[0].exists());

        let dest = tmp.join("restored");
        let n = unpack_tar_zst(&report.outputs[0], &dest).unwrap();
        assert_eq!(n, 2);
        assert!(dest.join("Release-06-2026/a.stl").exists());
        assert!(dest.join("Release-06-2026/sub/b.stl").exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn skips_apple_artifacts_at_pack_time() {
        let tmp = std::env::temp_dir().join("minihoard-pack-apple");
        let _ = std::fs::remove_dir_all(&tmp);
        let src = tmp.join("creator-06-2026").join("Release");
        make_tree(&src);
        // Finder cruft that may reappear between download and pack.
        std::fs::write(src.join(".DS_Store"), b"junk").unwrap();
        std::fs::write(src.join("._a.stl"), b"junk").unwrap();
        std::fs::create_dir_all(src.join("__MACOSX")).unwrap();
        std::fs::write(src.join("__MACOSX/ignore"), b"junk").unwrap();
        let out = tmp.join("out");

        let report = pack_dir(&src, &out, &PackOptions::default(), None, |_| {}).unwrap();
        // measure() and the sidecar agree, counting only the two real .stl files.
        assert_eq!(report.file_count, 2);
        let sidecar: Sidecar =
            serde_json::from_str(&std::fs::read_to_string(report.sidecar.unwrap()).unwrap())
                .unwrap();
        assert_eq!(sidecar.file_count, 2);
        assert!(
            !sidecar.entries.iter().any(|e| e.path.contains(".DS_Store")
                || e.path.contains("._")
                || e.path.contains("__MACOSX")),
            "apple artifacts leaked into sidecar: {:?}",
            sidecar.entries.iter().map(|e| &e.path).collect::<Vec<_>>()
        );

        // And they're absent from the actual archive on restore.
        let dest = tmp.join("restored");
        unpack_tar_zst(&report.outputs[0], &dest).unwrap();
        assert!(dest.join("Release/a.stl").exists());
        assert!(!dest.join("Release/.DS_Store").exists());
        assert!(!dest.join("Release/__MACOSX").exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn writes_sidecar_index() {
        let tmp = std::env::temp_dir().join("minihoard-pack-sidecar");
        let _ = std::fs::remove_dir_all(&tmp);
        // Parent folder matches {creator}-{MM-YYYY} so it's parsed into metadata.
        let src = tmp.join("one_page_rules-06-2026").join("DragonKnight");
        make_tree(&src);
        std::fs::write(src.join("render.png"), vec![0u8; 100]).unwrap();
        let out = tmp.join("out");

        let report = pack_dir(&src, &out, &PackOptions::default(), None, |_| {}).unwrap();
        let sidecar = report.sidecar.expect("sidecar written");
        assert!(sidecar.to_str().unwrap().ends_with("DragonKnight.tar.zst.json"));

        let parsed: Sidecar =
            serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
        assert_eq!(parsed.creator.as_deref(), Some("one_page_rules"));
        assert_eq!(parsed.release_month.as_deref(), Some("06-2026"));
        assert_eq!(parsed.file_count, 3);
        assert!(parsed.entries.iter().any(|e| e.kind == "model"));
        assert!(parsed.entries.iter().any(|e| e.kind == "image"));
        assert_eq!(parsed.volumes, vec!["DragonKnight.tar.zst".to_string()]);

        // Removal cleans up the archive + sidecar.
        let removed = remove_archive_files(&report.outputs[0]).unwrap();
        assert_eq!(removed, 2); // archive + sidecar
        assert!(!report.outputs[0].exists());
        assert!(!sidecar.exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn removes_all_split_volumes() {
        let tmp = std::env::temp_dir().join("minihoard-pack-rmsplit");
        let _ = std::fs::remove_dir_all(&tmp);
        let src = tmp.join("Big-06-2026");
        std::fs::create_dir_all(&src).unwrap();
        for i in 0..6u64 {
            let mut state = i.wrapping_add(1).wrapping_mul(0x9E3779B97F4A7C15);
            let data: Vec<u8> = (0..16384)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    (state & 0xff) as u8
                })
                .collect();
            std::fs::write(src.join(format!("f{i}.bin")), data).unwrap();
        }
        let out = tmp.join("out");
        let opts = PackOptions {
            format: PackFormat::TarZst,
            level: 1,
            split_bytes: Some(16384),
            write_sidecar: true,
        };
        let report = pack_dir(&src, &out, &opts, None, |_| {}).unwrap();
        let volumes = report.outputs.len();
        assert!(volumes > 1);

        // Pass the .001 volume; all siblings + sidecar should go.
        let removed = remove_archive_files(&report.outputs[0]).unwrap();
        assert_eq!(removed, volumes + 1);
        for v in &report.outputs {
            assert!(!v.exists());
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn zip_round_trip() {
        let tmp = std::env::temp_dir().join("minihoard-pack-zip");
        let _ = std::fs::remove_dir_all(&tmp);
        let src = tmp.join("Release-06-2026");
        make_tree(&src);
        let out = tmp.join("out");

        let opts = PackOptions {
            format: PackFormat::Zip,
            ..Default::default()
        };
        let report = pack_dir(&src, &out, &opts, None, |_| {}).unwrap();
        assert_eq!(report.outputs.len(), 1);
        assert!(report.outputs[0].to_str().unwrap().ends_with(".zip"));

        let dest = tmp.join("restored");
        let r = crate::unpack::unpack_zip_into(&report.outputs[0], &dest).unwrap();
        assert_eq!(r.files_written, 2);
        assert!(dest.join("Release-06-2026/a.stl").exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn split_round_trip() {
        let tmp = std::env::temp_dir().join("minihoard-pack-split");
        let _ = std::fs::remove_dir_all(&tmp);
        let src = tmp.join("Big-06-2026");
        std::fs::create_dir_all(&src).unwrap();
        // Incompressible data (xorshift) so zstd output still spans volumes.
        for i in 0..8u64 {
            let mut state = i.wrapping_add(1).wrapping_mul(0x9E3779B97F4A7C15);
            let data: Vec<u8> = (0..16384)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    (state & 0xff) as u8
                })
                .collect();
            std::fs::write(src.join(format!("f{i}.bin")), data).unwrap();
        }
        let out = tmp.join("out");
        let opts = PackOptions {
            format: PackFormat::TarZst,
            level: 1,
            split_bytes: Some(16384), // small volumes to force a split
            write_sidecar: true,
        };
        let report = pack_dir(&src, &out, &opts, None, |_| {}).unwrap();
        assert!(
            report.outputs.len() > 1,
            "expected multiple volumes, got {}",
            report.outputs.len()
        );
        // First volume name ends in .001
        assert!(report.outputs[0].to_str().unwrap().ends_with(".001"));

        let dest = tmp.join("restored");
        let n = unpack_tar_zst(&report.outputs[0], &dest).unwrap();
        assert_eq!(n, 8);
        assert!(dest.join("Big-06-2026/f7.bin").exists());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
