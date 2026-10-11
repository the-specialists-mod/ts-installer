//! Finding Half-Life installs in Steam libraries.

use std::path::{Path, PathBuf};

/// Steam install directories that exist on this machine.
fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(p) = std::env::var_os(var) {
                roots.push(PathBuf::from(p).join("Steam"));
            }
        }
    } else if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".local/share/Steam"));
        roots.push(home.join(".steam/steam"));
        roots.push(home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"));
    }
    roots.retain(|r| r.is_dir());
    roots
}

/// Library folders listed in `steamapps/libraryfolders.vdf` (lines like `"path" "D:\\Games"`).
fn library_folders(steam: &Path) -> Vec<PathBuf> {
    let mut out = vec![steam.to_path_buf()];
    let Ok(vdf) = std::fs::read_to_string(steam.join("steamapps/libraryfolders.vdf")) else { return out };
    for line in vdf.lines() {
        let mut parts = line.split('"').filter(|s| !s.trim().is_empty());
        if parts.next() == Some("path")
            && let Some(p) = parts.next()
        {
            out.push(PathBuf::from(p.replace("\\\\", "\\")));
        }
    }
    out
}

/// Half-Life installs found in Steam libraries, without duplicates.
pub fn half_life_installs() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for steam in steam_roots() {
        for lib in library_folders(&steam) {
            let hl = lib.join("steamapps").join("common").join("Half-Life");
            if crate::install::is_half_life_dir(&hl) {
                let canon = hl.canonicalize().unwrap_or(hl);
                if !found.contains(&canon) {
                    found.push(canon);
                }
            }
        }
    }
    found
}

/// A default directory for a new Xash3D install.
pub fn default_xash_dir() -> PathBuf {
    if cfg!(windows) {
        dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("C:\\")).join("TheSpecialists")
    } else {
        dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("the-specialists")
    }
}
