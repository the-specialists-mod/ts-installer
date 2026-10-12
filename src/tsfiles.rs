//! Checking an install's TS 3.0 files against the release (`ts-3-final.exe`), and repairing or
//! resetting them from it.
//!
//! The installer is checked against its known MD5, and every `ts/` file in it is hashed (SHA-256)
//! and compared with the installed copy. Files the Rust port replaces are compared with the
//! copies kept in `ts/.tsinstaller-originals` while the port is installed. Settings the game and
//! the launcher rewrite are reported apart and left alone by a repair. Files the release doesn't
//! have (custom maps, models, logs) are never looked at.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::clickteam::{Entry, Installer};
use crate::install::{self, Installation, LibChoice, ORIGINALS_DIR, PORT_REPLACES, TsLibs};
use crate::progress::Progress;
use crate::sources;

/// Files in `ts/` that are meant to change: game settings, server settings, the bot list.
pub const SETTINGS: &[&str] =
    &["config.cfg", "server.cfg", "listenserver.cfg", "mapcycle.txt", "motd.txt", "bots.txt", "user.scr", "settings.scr"];

/// How an install's TS 3.0 files compare with the release.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TsCheck {
    pub matching: usize,
    /// Damaged files, relative to `ts/`.
    pub different: Vec<String>,
    pub missing: Vec<String>,
    /// Settings files that differ from the release's defaults (expected).
    pub settings_changed: Vec<String>,
}

impl TsCheck {
    pub fn ok(&self) -> bool {
        self.different.is_empty() && self.missing.is_empty()
    }

    /// The damaged and missing files.
    pub fn broken(&self) -> Vec<String> {
        self.different.iter().chain(&self.missing).cloned().collect()
    }
}

/// `ts-3-final.exe`, downloaded (or taken from the cache) and checked against its MD5.
fn release_installer(progress: &Progress) -> Result<Installer> {
    let path = sources::download_ts_installer(progress)?;
    Installer::open(&path)
}

/// The installer's `ts/` entries with their paths inside `ts`.
fn ts_entries(inst: &Installer) -> Vec<(String, Entry)> {
    inst.entries
        .iter()
        .filter_map(|e| {
            let rel = e.relative_path()?;
            let inner = rel.strip_prefix("ts").ok()?;
            let name = inner.to_string_lossy().replace('\\', "/");
            (!name.is_empty()).then(|| (name, e.clone()))
        })
        .collect()
}

/// Where the installed copy of release file `name` lives: in `ts/`, or for files the Rust port
/// replaced, in the kept originals.
fn installed_path(ts: &Path, name: &str, port: bool) -> PathBuf {
    if port && PORT_REPLACES.contains(&name) { ts.join(ORIGINALS_DIR).join(name) } else { ts.join(name) }
}

struct HashWriter(Sha256);

impl Write for HashWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn entry_sha256(inst: &mut Installer, entry: &Entry) -> Result<String> {
    let mut h = HashWriter(Sha256::new());
    inst.extract(entry, &mut h)?;
    Ok(h.0.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Compares the TS 3.0 files in `root/ts` with the release.
pub fn validate(root: &Path, progress: &Progress) -> Result<TsCheck> {
    progress.step(0, 2, "Check ts-3-final.exe");
    let mut inst = release_installer(progress)?;
    let ts = root.join("ts");
    let port = install::ts_libs(&ts) == Some(TsLibs::RustPort);
    let entries = ts_entries(&inst);
    progress.step(1, 2, format!("Compare SHA-256 of {} TS 3.0 files", entries.len()));
    let total: u64 = entries.iter().map(|(_, e)| e.size as u64).sum();
    let mut done = 0;
    let mut check = TsCheck::default();
    for (name, entry) in &entries {
        progress.check_cancelled()?;
        // For a file the port may replace, the kept copy is what switching back restores, so it
        // is the one checked; without one (the port left the file alone) the file in ts/ is.
        let kept = installed_path(&ts, name, port);
        let candidates: Vec<PathBuf> = if kept.is_file() { vec![kept] } else { vec![ts.join(name)] };
        let present: Vec<&PathBuf> = candidates.iter().filter(|p| p.is_file()).collect();
        let settings = SETTINGS.contains(&name.as_str());
        let mut want = None;
        let mut matches = false;
        for path in &present {
            if std::fs::metadata(path)?.len() != entry.size as u64 {
                continue;
            }
            if want.is_none() {
                want = Some(entry_sha256(&mut inst, entry)?);
            }
            if want.as_deref() == Some(crate::net::file_sha256(path)?.as_str()) {
                matches = true;
                break;
            }
        }
        if present.is_empty() {
            progress.log(format!("missing: {name}"));
            check.missing.push(name.clone());
        } else if matches {
            check.matching += 1;
        } else if settings {
            check.settings_changed.push(name.clone());
        } else {
            progress.log(format!("differs: {name}"));
            check.different.push(name.clone());
        }
        done += entry.size as u64;
        progress.bytes(done, Some(total));
    }
    progress.log(format!(
        "{} files match, {} differ, {} missing, {} settings changed",
        check.matching,
        check.different.len(),
        check.missing.len(),
        check.settings_changed.len()
    ));
    Ok(check)
}

/// Extracts release files `names` (paths inside `ts`) to where the install keeps them.
fn extract(inst: &mut Installer, ts: &Path, names: &[String], port: bool, progress: &Progress) -> Result<usize> {
    let entries = ts_entries(inst);
    let mut n = 0;
    for (name, entry) in entries.iter().filter(|(name, _)| names.contains(name)) {
        progress.check_cancelled()?;
        let path = installed_path(ts, name, port);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = std::io::BufWriter::new(std::fs::File::create(&path).with_context(|| format!("create {}", path.display()))?);
        inst.extract(entry, &mut file)?;
        file.flush()?;
        n += 1;
    }
    Ok(n)
}

/// Validates the TS 3.0 files and extracts the damaged and missing ones again. Settings are left
/// as they are. Returns the check made before repairing.
pub fn repair(root: &Path, progress: &Progress) -> Result<TsCheck> {
    let check = validate(root, progress)?;
    let broken = check.broken();
    if !broken.is_empty() {
        let ts = root.join("ts");
        let port = install::ts_libs(&ts) == Some(TsLibs::RustPort);
        let mut inst = release_installer(progress)?;
        let n = extract(&mut inst, &ts, &broken, port, progress)?;
        progress.log(format!("restored {n} files from ts-3-final.exe"));
    }
    Ok(check)
}

/// Resets `install` to a clean TS 3.0: the original game libraries (with the engine they need)
/// and every release file extracted again, settings included. Files the release doesn't have,
/// like custom maps and player models, stay.
pub fn reset(install: &Installation, progress: &Progress) -> Result<Installation> {
    let ts = install.root.join("ts");
    let install = if install::ts_libs(&ts) == Some(TsLibs::Original) {
        install.clone()
    } else {
        install::switch_libs(install, LibChoice::Original, None, progress)?
    };
    // Back on the originals: drop the kept copies, the release has them.
    let _ = std::fs::remove_dir_all(ts.join(ORIGINALS_DIR));
    let mut inst = release_installer(progress)?;
    let names: Vec<String> = ts_entries(&inst).into_iter().map(|(n, _)| n).collect();
    progress.log(format!("extracting {} TS 3.0 files", names.len()));
    let n = extract(&mut inst, &ts, &names, false, progress)?;
    progress.log(format!("reset {n} files to the TS 3.0 release"));
    Ok(install)
}
