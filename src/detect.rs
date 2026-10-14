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
                let canon = dunce::canonicalize(&hl).unwrap_or(hl);
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

/// Where new Xash3D installations go when the launcher was put in place by the Windows installer
/// (its uninstaller sits beside the executable): `<launcher folder>/installs`. `None` for a
/// launcher run from anywhere else, or installed somewhere this user can't write.
pub fn installed_launcher_installs_dir() -> Option<PathBuf> {
    let exe = dunce::canonicalize(std::env::current_exe().ok()?).ok()?;
    installs_dir_beside(exe.parent()?)
}

fn installs_dir_beside(dir: &Path) -> Option<PathBuf> {
    if !dir.join("unins000.exe").is_file() {
        return None;
    }
    // Program Files, for an install for all users, is read-only without administrator rights.
    let probe = dir.join(".ts-launcher-write-test");
    std::fs::write(&probe, b"").ok()?;
    let _ = std::fs::remove_file(&probe);
    Some(dir.join("installs"))
}

/// The first free installation name in `dir`: "The Specialists", then "The Specialists 2", ...
pub fn free_install_name(dir: &Path) -> String {
    (1..)
        .map(|n| if n == 1 { "The Specialists".to_owned() } else { format!("The Specialists {n}") })
        .find(|name| !dir.join(name).exists())
        .expect("a free name")
}

/// Why `name` can't be a folder name on Windows or Linux, if it can't.
pub fn folder_name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("enter a name for the installation");
    }
    if name.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control()) {
        return Some("the name can't contain < > : \" / \\ | ? or *");
    }
    if name.ends_with('.') || name.ends_with(' ') || name.starts_with(' ') || name == "." || name == ".." {
        return Some("the name can't start with a space or end with a space or a dot");
    }
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    reserved.then_some("that name is reserved on Windows")
}

/// The first detected Half-Life install that already has TS in its `ts` folder, as an
/// installation the launcher can use without running setup.
pub fn existing_ts_install(half_life: &[PathBuf]) -> Option<crate::install::Installation> {
    let root = half_life.iter().find(|hl| crate::install::ts_libs(&hl.join("ts")).is_some())?;
    Some(crate::install::Installation { root: root.clone(), runtime: crate::install::Runtime::GoldSrc })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_dir_needs_the_uninstaller() {
        let dir = std::env::temp_dir().join(format!("ts-launcher-installed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(installs_dir_beside(&dir), None);
        std::fs::write(dir.join("unins000.exe"), b"").unwrap();
        assert_eq!(installs_dir_beside(&dir), Some(dir.join("installs")));
        assert!(!dir.join(".ts-launcher-write-test").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn free_names_skip_existing_folders() {
        let dir = std::env::temp_dir().join(format!("ts-launcher-names-{}", std::process::id()));
        assert_eq!(free_install_name(&dir), "The Specialists");
        std::fs::create_dir_all(dir.join("The Specialists")).unwrap();
        std::fs::create_dir_all(dir.join("The Specialists 2")).unwrap();
        assert_eq!(free_install_name(&dir), "The Specialists 3");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name_problem("The Specialists 2"), None);
        assert_eq!(folder_name_problem("clan.server"), None);
        assert!(folder_name_problem("  ").is_some());
        assert!(folder_name_problem("a/b").is_some());
        assert!(folder_name_problem("what?").is_some());
        assert!(folder_name_problem("dots.").is_some());
        assert!(folder_name_problem("con").is_some());
        assert!(folder_name_problem("COM1.txt").is_some());
        assert_eq!(folder_name_problem("COMMS"), None);
    }
}
