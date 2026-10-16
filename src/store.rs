//! The launcher's saved state: every known installation and its launch options.
//!
//! It lives in `~/.tsinstaller` on Linux and `%APPDATA%\tsinstaller` on Windows (override with
//! the `TSINSTALLER_HOME` environment variable), in `installations.json`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::install::{Installation, Runtime};

/// The launcher's data directory.
pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("TSINSTALLER_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    if cfg!(windows) {
        dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("tsinstaller")
    } else {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".tsinstaller")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub instances: Vec<Instance>,
    /// `id` of the installation used last.
    pub last_used: Option<String>,
    /// Server addresses (`host:port`) saved in the server browser.
    pub favorite_servers: Vec<String>,
    /// Optional Steam Web API key, for listing servers from Steam's master server when the Steam
    /// client isn't running.
    pub steam_api_key: String,
}

/// One installation the launcher knows about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub install: Installation,
    /// Unix time the installation was added.
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub last_played: Option<u64>,
    #[serde(default)]
    pub launch: LaunchOptions,
    /// The Rust port release this installation stays on. `None` follows the latest release:
    /// the launcher updates the port to it on its own.
    #[serde(default)]
    pub port_tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaunchOptions {
    pub windowed: bool,
    /// Opens the developer console (`-console`).
    pub console: bool,
    /// Extra command-line arguments, split on whitespace.
    pub extra_args: String,
    /// Wine executable for Windows builds on Linux.
    pub wine: String,
    /// `WINEPREFIX`; empty uses Wine's default.
    pub wine_prefix: String,
    /// Play hosts a listen server on `map` with `bots`, using the other "Start a server" settings.
    pub local_server: bool,
    /// The local server's map; empty picks a default from `ts/maps`.
    pub map: String,
    /// Bots that join the local server.
    pub bots: u32,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            windowed: false,
            console: true,
            extra_args: String::new(),
            wine: "wine".into(),
            wine_prefix: String::new(),
            local_server: false,
            map: String::new(),
            bots: 0,
        }
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Instance {
    pub fn new(install: Installation) -> Self {
        let name = default_name(&install);
        Self {
            id: format!("{:x}", now() * 1000 + u64::from(std::process::id() % 1000)),
            name,
            install,
            created: now(),
            last_played: None,
            launch: LaunchOptions::default(),
            port_tag: None,
        }
    }

    pub fn ts_dir(&self) -> PathBuf {
        self.install.root.join("ts")
    }
}

/// "Half-Life (Steam)" or "Xash3D: <folder name>".
fn default_name(install: &Installation) -> String {
    let folder = install.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match install.runtime {
        Runtime::GoldSrc if install.root.components().any(|c| c.as_os_str().eq_ignore_ascii_case("steamapps")) => {
            "Half-Life (Steam)".into()
        }
        Runtime::GoldSrc => format!("Half-Life: {folder}"),
        Runtime::Xash(_) => format!("Xash3D: {folder}"),
    }
}

impl Store {
    pub fn path() -> PathBuf {
        home().join("installations.json")
    }

    /// Loads the saved store; an unreadable file gives an empty store. Imports the single
    /// installation saved by launcher 0.1 (`<config dir>/ts-launcher/config.json`) once.
    pub fn load() -> Self {
        let mut store: Self = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).map_err(|e| log::warn!("ignoring bad {}: {e}", Self::path().display())).ok())
            .unwrap_or_default();
        if !Self::path().exists()
            && let Some((install, launch)) = legacy_installation()
        {
            let id = store.add(install);
            if let Some(inst) = store.get_mut(&id) {
                inst.launch = launch;
            }
            let _ = store.save();
        }
        store
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("write {}", path.display()))
    }

    pub fn get(&self, id: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Instance> {
        self.instances.iter_mut().find(|i| i.id == id)
    }

    /// The installation at `root`, if one is registered.
    pub fn find_root(&self, root: &Path) -> Option<&Instance> {
        self.instances.iter().find(|i| crate::sources::same_path(&i.install.root, root) || i.install.root == root)
    }

    /// Registers `install`, replacing an existing entry for the same folder (and keeping its id,
    /// name and launch options). Returns the id.
    pub fn add(&mut self, install: Installation) -> String {
        let existing = self.find_root(&install.root).map(|i| i.id.clone());
        let id = match existing.and_then(|id| self.get_mut(&id)) {
            Some(inst) => {
                inst.install = install;
                inst.id.clone()
            }
            None => {
                let mut inst = Instance::new(install);
                while self.get(&inst.id).is_some() {
                    inst.id.push('0');
                }
                let id = inst.id.clone();
                self.instances.push(inst);
                id
            }
        };
        self.last_used = Some(id.clone());
        id
    }

    /// Forgets an installation (the files stay on disk).
    pub fn remove(&mut self, id: &str) {
        self.instances.retain(|i| i.id != id);
        if self.last_used.as_deref() == Some(id) {
            self.last_used = None;
        }
    }
}

#[derive(Deserialize)]
struct LegacyConfig {
    installation: Option<Installation>,
    #[serde(default)]
    launch: LaunchOptions,
}

fn legacy_installation() -> Option<(Installation, LaunchOptions)> {
    let path = dirs::config_dir()?.join("ts-launcher").join("config.json");
    let old: LegacyConfig = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    Some((old.installation?, old.launch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_replaces_same_root() {
        let mut s = Store::default();
        let a = s.add(Installation { root: "/x/Half-Life".into(), runtime: Runtime::GoldSrc });
        let b = s.add(Installation { root: "/y/ts".into(), runtime: Runtime::GoldSrc });
        let a2 = s.add(Installation { root: "/x/Half-Life".into(), runtime: Runtime::GoldSrc });
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_eq!(s.instances.len(), 2);
        assert_eq!(s.last_used.as_deref(), Some(a.as_str()));
        s.remove(&a);
        assert_eq!(s.instances.len(), 1);
    }

    #[test]
    fn round_trips() {
        let mut s = Store::default();
        s.add(Installation { root: "/x".into(), runtime: Runtime::GoldSrc });
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Store>(&json).unwrap(), s);
    }
}
