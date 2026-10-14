//! The HD pack: HD weapon and player models and weapon sounds collected from the TSMOD.NET client,
//! published as a release of the-specialists-mod/tsmod-hdpack.
//!
//! Its archive holds a `ts/` directory copied over the game's. The TS 3.0 files it replaces are
//! kept in `ts/.tsinstaller-hdpack-originals` and a manifest lists every file it wrote, so
//! [`remove`] can put the install back as it was.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::net;
use crate::progress::Progress;

pub const REPO: &str = "the-specialists-mod/tsmod-hdpack";
pub const TAG: &str = "v1.0.0";
/// The release archive's directory that goes over `ts/`.
const ARCHIVE_PREFIX: &str = "ts";
/// Where the TS 3.0 files the pack replaces are kept, inside `ts`.
pub(crate) const ORIGINALS_DIR: &str = ".tsinstaller-hdpack-originals";
/// The [`Manifest`] of the pack in `ts`.
const MANIFEST: &str = ".tsinstaller-hdpack.json";

pub fn asset(tag: &str) -> String {
    format!("tsmod-hdpack-{tag}.zip")
}

/// The pack release in `ts` and the files it wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub tag: String,
    /// Every file the pack wrote, relative to `ts` with `/` separators.
    pub files: Vec<String>,
    /// The files in `files` that replaced a TS file, kept in [`ORIGINALS_DIR`].
    pub replaced: Vec<String>,
}

/// The pack installed in the game directory `ts`, if any.
pub fn manifest(ts: &Path) -> Option<Manifest> {
    serde_json::from_str(&std::fs::read_to_string(ts.join(MANIFEST)).ok()?).ok()
}

/// Where the TS 3.0 copy of `name` (relative to `ts`) is while the pack is installed: in the kept
/// originals when the pack replaced it.
pub fn original_path(ts: &Path, name: &str) -> Option<PathBuf> {
    let m = manifest(ts)?;
    m.replaced.iter().any(|r| r == name).then(|| ts.join(ORIGINALS_DIR).join(name))
}

fn rel_name(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

/// Downloads the pack release (checked against its SHA-256) and copies it over `root/ts`, first
/// keeping the TS files it replaces. Installing over an installed pack keeps the first originals.
pub fn install(root: &Path, progress: &Progress) -> Result<Manifest> {
    let ts = root.join("ts");
    if !ts.join("liblist.gam").is_file() {
        bail!("{} has no TS game directory (ts/liblist.gam)", root.display());
    }
    let name = asset(TAG);
    let archive = net::cache_dir().join(&name);
    net::cached_github_asset(REPO, TAG, &name, &archive, progress)?;

    let unpacked = net::cache_dir().join("hdpack-unpacked");
    let _ = std::fs::remove_dir_all(&unpacked);
    progress.log(format!("unpacking {name}"));
    let n = crate::archive::extract_zip(&archive, ARCHIVE_PREFIX, &unpacked, progress)?;
    if n == 0 {
        bail!("{name} has no {ARCHIVE_PREFIX}/ directory");
    }

    let previous = manifest(&ts);
    let mut replaced = previous.as_ref().map(|m| m.replaced.clone()).unwrap_or_default();
    let previous_files = previous.map(|m| m.files).unwrap_or_default();
    let originals = ts.join(ORIGINALS_DIR);
    let mut files = Vec::new();
    for rel in crate::archive::list_files(&unpacked)? {
        let name = rel_name(&rel);
        // A file an earlier pack wrote is the pack's, not the game's: don't keep it as an original.
        if ts.join(&rel).is_file() && !previous_files.contains(&name) && !replaced.contains(&name) {
            let kept = originals.join(&rel);
            std::fs::create_dir_all(kept.parent().unwrap_or(&originals))?;
            std::fs::copy(ts.join(&rel), &kept).with_context(|| format!("back up {name}"))?;
            replaced.push(name.clone());
        }
        files.push(name);
    }
    progress.log(format!("kept the {} TS files the HD pack replaces in {}", replaced.len(), originals.display()));
    let n = crate::archive::copy_dir(&unpacked, &ts, progress)?;
    let _ = std::fs::remove_dir_all(&unpacked);
    progress.log(format!("{n} HD pack files installed"));

    files.sort();
    replaced.sort();
    let m = Manifest { tag: TAG.to_owned(), files, replaced };
    std::fs::write(ts.join(MANIFEST), serde_json::to_string_pretty(&m)?)?;
    Ok(m)
}

/// Puts back the TS files the pack replaced and deletes the ones it added.
pub fn remove(root: &Path, progress: &Progress) -> Result<usize> {
    let ts = root.join("ts");
    let m = manifest(&ts).context("the HD pack isn't installed")?;
    let originals = ts.join(ORIGINALS_DIR);
    for name in &m.files {
        progress.check_cancelled()?;
        let target = ts.join(name);
        let kept = originals.join(name);
        if m.replaced.contains(name) && kept.is_file() {
            let _ = std::fs::remove_file(&target);
            std::fs::rename(&kept, &target)
                .or_else(|_| std::fs::copy(&kept, &target).map(|_| ()))
                .with_context(|| format!("restore {name}"))?;
        } else {
            let _ = std::fs::remove_file(&target);
        }
    }
    let _ = std::fs::remove_dir_all(&originals);
    std::fs::remove_file(ts.join(MANIFEST))?;
    progress.log(format!("removed the HD pack: {} files restored, {} removed", m.replaced.len(), m.files.len() - m.replaced.len()));
    Ok(m.files.len())
}

/// Drops the pack when every TS file is about to be extracted again: deletes the files it added
/// (the extraction replaces the rest) and forgets the kept originals.
pub(crate) fn forget(ts: &Path) {
    if let Some(m) = manifest(ts) {
        for name in m.files.iter().filter(|f| !m.replaced.contains(f)) {
            let _ = std::fs::remove_file(ts.join(name));
        }
    }
    let _ = std::fs::remove_dir_all(ts.join(ORIGINALS_DIR));
    let _ = std::fs::remove_file(ts.join(MANIFEST));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_restores_replaced_files_and_drops_added_ones() {
        let dir = std::env::temp_dir().join(format!("ts-launcher-hdpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ts = dir.join("ts");
        std::fs::create_dir_all(ts.join(ORIGINALS_DIR).join("models")).unwrap();
        std::fs::create_dir_all(ts.join("models")).unwrap();
        std::fs::create_dir_all(ts.join("sound")).unwrap();
        std::fs::write(ts.join("models/v_ak47.mdl"), "hd").unwrap();
        std::fs::write(ts.join(ORIGINALS_DIR).join("models/v_ak47.mdl"), "stock").unwrap();
        std::fs::write(ts.join("sound/new.wav"), "hd").unwrap();
        let m = Manifest {
            tag: TAG.into(),
            files: vec!["models/v_ak47.mdl".into(), "sound/new.wav".into()],
            replaced: vec!["models/v_ak47.mdl".into()],
        };
        std::fs::write(ts.join(MANIFEST), serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(original_path(&ts, "models/v_ak47.mdl"), Some(ts.join(ORIGINALS_DIR).join("models/v_ak47.mdl")));

        remove(&dir, &Progress::silent()).unwrap();
        assert_eq!(std::fs::read_to_string(ts.join("models/v_ak47.mdl")).unwrap(), "stock");
        assert!(!ts.join("sound/new.wav").exists());
        assert!(!ts.join(ORIGINALS_DIR).exists());
        assert!(manifest(&ts).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
