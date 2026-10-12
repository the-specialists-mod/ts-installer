//! The download cache: what is in it, and removing entries. Installs, repairs, resets and
//! validation use these files instead of downloading them again.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};

use crate::net;
use crate::sources::{self, XashBuild};

#[derive(Debug, Clone, PartialEq)]
pub struct CacheEntry {
    pub path: PathBuf,
    /// The path inside the cache, with `/` separators.
    pub name: String,
    /// What the file is, for people.
    pub what: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

pub fn dir() -> PathBuf {
    net::cache_dir()
}

/// What a cached file (named relative to the cache) is.
pub fn describe(name: &str) -> String {
    if let Some(done) = name.strip_suffix(".part") {
        let what = describe(done);
        // "The Specialists…" and "Half-Life…" keep their capitals; "Engine: …" doesn't need one.
        let what = if what.starts_with("Engine") || what.starts_with("Rust") || what.starts_with("Other") {
            what[..1].to_lowercase() + &what[1..]
        } else {
            what
        };
        return format!("Incomplete download of {what} (resumes next time)");
    }
    let file = name.rsplit('/').next().unwrap_or(name);
    if name == "port/unpacked" {
        return "Unpacked Rust port files (temporary)".into();
    }
    if file == sources::TS_INSTALLER_NAME {
        return "The Specialists 3.0 installer (installs, repairs and resets TS)".into();
    }
    if file == sources::XASH_TS_CACHE_NAME {
        return "Half-Life data from xash3d-ts".into();
    }
    let builds = [XashBuild::WindowsI386, XashBuild::WindowsAmd64, XashBuild::LinuxAmd64, XashBuild::LinuxI386];
    if let Some(b) = builds.iter().find(|b| b.asset() == file) {
        return format!("Engine: {}", b.label());
    }
    if let Some(rest) = file.strip_prefix("the-specialists-rs-") {
        let rest = rest.trim_end_matches(".tar.gz").trim_end_matches(".zip");
        if let Some((tag, build)) = rest.split_once('-') {
            return format!("Rust port {tag} ({build})");
        }
    }
    "Other file".into()
}

fn dir_size(dir: &Path) -> u64 {
    crate::archive::list_files(dir)
        .map(|f| f.iter().filter_map(|r| std::fs::metadata(dir.join(r)).ok()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

/// Everything in the cache: its files, the port archives in `port/`, and other directories
/// as single entries. Newest first.
pub fn entries() -> Vec<CacheEntry> {
    let root = dir();
    let mut out = Vec::new();
    let mut add = |path: PathBuf, name: String| {
        let meta = std::fs::metadata(&path).ok();
        let size = match &meta {
            Some(m) if m.is_dir() => dir_size(&path),
            Some(m) => m.len(),
            None => 0,
        };
        let what = describe(&name);
        out.push(CacheEntry { modified: meta.and_then(|m| m.modified().ok()), path, name, what, size });
    };
    for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == "port" && e.path().is_dir() {
            for p in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
                add(p.path(), format!("port/{}", p.file_name().to_string_lossy()));
            }
        } else {
            add(e.path(), name);
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.modified));
    out
}

/// Deletes one entry (a file or a directory) from the cache.
pub fn remove(path: &Path) -> Result<()> {
    anyhow::ensure!(path.starts_with(dir()), "{} is not in the cache", path.display());
    if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }
        .with_context(|| format!("delete {}", path.display()))
}

/// Empties the cache.
pub fn clear() -> Result<()> {
    for e in entries() {
        remove(&e.path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_cached_files() {
        assert_eq!(describe("ts-3-final.exe"), "The Specialists 3.0 installer (installs, repairs and resets TS)");
        assert_eq!(describe("xash3d-fwgs-linux-i386.tar.gz"), "Engine: Xash3D FWGS (Linux, 32-bit)");
        assert_eq!(describe("port/the-specialists-rs-v3.0.1-linux-x86_64-debug.tar.gz"), "Rust port v3.0.1 (linux-x86_64-debug)");
        assert_eq!(describe("xash3d-ts-release.zip.part"), "Incomplete download of Half-Life data from xash3d-ts (resumes next time)");
        assert_eq!(describe("port/unpacked"), "Unpacked Rust port files (temporary)");
    }
}
