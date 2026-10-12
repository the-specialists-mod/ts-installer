//! Extracting the archive formats the sources come in, and copying directories.
//!
//! Every extractor maps entry names through [`safe_relative`], so a hostile archive cannot write
//! outside the destination.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};

use crate::progress::Progress;

/// `name` as a relative path below the destination, with the first `strip` components removed.
/// `None` for names that are empty after stripping or would escape the destination.
pub fn safe_relative(name: &str, strip: usize) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".").skip(strip) {
        if part.contains(':') || !matches!(Path::new(part).components().next(), Some(Component::Normal(_))) {
            return None;
        }
        out.push(part);
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn write_file(target: &Path, mut data: impl Read, executable: bool) -> Result<u64> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    // Replace rather than truncate: the old file may be a symlink into a shared directory.
    let _ = fs::remove_file(target);
    let mut out = BufWriter::new(File::create(target).with_context(|| format!("create {}", target.display()))?);
    let n = io::copy(&mut data, &mut out)?;
    out.flush()?;
    set_executable(target, executable)?;
    Ok(n)
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if executable {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_: &Path, _: bool) -> Result<()> {
    Ok(())
}

/// Extracts a `.tar.gz`, dropping the first `strip` path components. Keeps symlinks (on Unix)
/// and executable bits.
pub fn extract_tar_gz(archive: &Path, dest: &Path, strip: usize, progress: &Progress) -> Result<usize> {
    let file = File::open(archive).with_context(|| format!("open {}", archive.display()))?;
    let total = file.metadata()?.len();
    let read = Arc::new(AtomicU64::new(0));
    let counter = CountingReader { inner: file, read: read.clone() };
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(counter));
    let mut count = 0;
    let entries = tar.entries()?;
    for entry in entries {
        progress.check_cancelled()?;
        let mut entry = entry?;
        let name = entry.path()?.to_string_lossy().into_owned();
        let Some(rel) = safe_relative(&name, strip) else { continue };
        let target = dest.join(&rel);
        match entry.header().entry_type() {
            tar::EntryType::Directory => fs::create_dir_all(&target)?,
            tar::EntryType::Symlink => {
                let link = entry.link_name()?.context("symlink without target")?.into_owned();
                // Only same-directory links, as in the FWGS release (libfoo.so -> libfoo.so.1).
                if link.components().count() == 1 {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let _ = fs::remove_file(&target);
                    symlink_or_copy(&link, &target)?;
                }
            }
            tar::EntryType::Regular => {
                let executable = entry.header().mode().map(|m| m & 0o111 != 0).unwrap_or(false);
                write_file(&target, &mut entry, executable)?;
                count += 1;
            }
            _ => {}
        }
        progress.bytes(read.load(Ordering::Relaxed), Some(total));
    }
    Ok(count)
}

/// Counts the compressed bytes read, for progress while the tar reader owns the stream.
struct CountingReader<R> {
    inner: R,
    read: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

#[cfg(unix)]
fn symlink_or_copy(link: &Path, target: &Path) -> Result<()> {
    std::os::unix::fs::symlink(link, target)?;
    Ok(())
}

#[cfg(not(unix))]
fn symlink_or_copy(link: &Path, target: &Path) -> Result<()> {
    let source = target.parent().unwrap_or(Path::new(".")).join(link);
    if source.exists() {
        fs::copy(source, target)?;
    }
    Ok(())
}

/// Extracts a `.7z` archive.
pub fn extract_7z(archive: &Path, dest: &Path, progress: &Progress) -> Result<usize> {
    let file = File::open(archive).with_context(|| format!("open {}", archive.display()))?;
    let mut reader =
        sevenz_rust2::ArchiveReader::new(file, sevenz_rust2::Password::empty()).with_context(|| format!("read {}", archive.display()))?;
    let total: u64 = reader.archive().files.iter().map(|f| f.size()).sum();
    let mut done = 0u64;
    let mut count = 0;
    let mut failure = None;
    reader.for_each_entries(|entry, data| {
        if let Err(e) = progress.check_cancelled() {
            failure = Some(e);
            return Ok(false);
        }
        let Some(rel) = safe_relative(entry.name(), 0) else { return Ok(true) };
        let target = dest.join(rel);
        if entry.is_directory() {
            fs::create_dir_all(&target)?;
            return Ok(true);
        }
        let executable = target.extension().is_none_or(|e| e == "so");
        match write_file(&target, data, executable) {
            Ok(n) => done += n,
            Err(e) => {
                failure = Some(e);
                return Ok(false);
            }
        }
        count += 1;
        progress.bytes(done, Some(total));
        Ok(true)
    })?;
    match failure {
        Some(e) => Err(e),
        None => Ok(count),
    }
}

/// Extracts the entries of a `.zip` below `prefix` (a `/`-separated directory inside the
/// archive, or `""` for everything) into `dest`, without the prefix.
pub fn extract_zip(archive: &Path, prefix: &str, dest: &Path, progress: &Progress) -> Result<usize> {
    let file = File::open(archive).with_context(|| format!("open {}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(io::BufReader::new(file)).with_context(|| format!("read {}", archive.display()))?;
    let prefix = prefix.trim_matches('/');
    let strip = prefix.split('/').filter(|p| !p.is_empty()).count();
    let wanted = |name: &str| prefix.is_empty() || name.trim_start_matches('/').starts_with(&format!("{prefix}/"));
    let mut total = 0u64;
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i)?;
        if wanted(&entry.name()?) {
            total += entry.size();
        }
    }
    let mut done = 0u64;
    let mut count = 0;
    for i in 0..zip.len() {
        progress.check_cancelled()?;
        let mut entry = zip.by_index(i)?;
        let name = entry.name()?.replace('\\', "/");
        if !wanted(&name) {
            continue;
        }
        let Some(rel) = safe_relative(&name, strip) else { continue };
        let target = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        let executable = entry.unix_mode().is_some_and(|m| m & 0o111 != 0);
        done += write_file(&target, &mut entry, executable)?;
        count += 1;
        progress.bytes(done, Some(total));
    }
    Ok(count)
}

/// Copies the contents of `src` into `dest`, overwriting files. Symlinks are followed.
pub fn copy_dir(src: &Path, dest: &Path, progress: &Progress) -> Result<usize> {
    let mut files = Vec::new();
    collect_files(src, Path::new(""), &mut files)?;
    let total: u64 = files.iter().map(|(_, len)| len).sum();
    let mut done = 0u64;
    for (rel, len) in &files {
        progress.check_cancelled()?;
        let from = src.join(rel);
        let to = dest.join(rel);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        let _ = fs::remove_file(&to);
        fs::copy(&from, &to).with_context(|| format!("copy {} to {}", from.display(), to.display()))?;
        done += len;
        progress.bytes(done, Some(total));
    }
    Ok(files.len())
}

/// Paths of every file under `dir`, relative to it.
pub fn list_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files(dir, Path::new(""), &mut files)?;
    Ok(files.into_iter().map(|(rel, _)| rel).collect())
}

fn collect_files(root: &Path, rel: &Path, out: &mut Vec<(PathBuf, u64)>) -> Result<()> {
    let dir = root.join(rel);
    for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let rel = rel.join(entry.file_name());
        let meta = fs::metadata(entry.path())?;
        if meta.is_dir() {
            collect_files(root, &rel, out)?;
        } else if meta.is_file() {
            out.push((rel, meta.len()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_relative_strips_and_rejects() {
        assert_eq!(safe_relative("xash/valve/maps/a.bsp", 1), Some(PathBuf::from("valve/maps/a.bsp")));
        assert_eq!(safe_relative("top/", 1), None);
        assert_eq!(safe_relative("top/../../etc/passwd", 1), None);
        assert_eq!(safe_relative("/abs/x", 0), Some(PathBuf::from("abs/x")));
        assert_eq!(safe_relative("C:/x", 0), None);
    }
}
