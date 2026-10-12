//! The install plan the setup wizard builds, and the code that carries it out.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::progress::Progress;
use crate::sources::{self, XashBuild};

/// Which engine runs TS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Engine {
    /// An existing Half-Life install: TS goes into its `ts` directory.
    HalfLife,
    /// Xash3D FWGS in the install directory with a `valve` directory beside it.
    Xash { engine: EngineSource, valve: ValveSource },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EngineSource {
    /// Download the Xash3D FWGS build the game libraries need.
    Download,
    /// Keep the engine already in the folder.
    Keep(XashBuild),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValveSource {
    /// Keep the `valve` directory already in the folder.
    Keep,
    /// The `valve` directory of the xash3d-ts release.
    XashTs,
    /// Copy `valve` from a local Half-Life install.
    CopyFrom(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TsSource {
    /// Keep the `ts` directory already in the folder.
    Keep,
    /// `ts-3-final.exe` from this repository's release, or its Internet Archive mirror.
    Download,
    /// A local copy of the installer `.exe`, or a directory holding a `ts` game directory.
    Local(PathBuf),
}

/// Which game libraries (`client` and server DLLs) TS runs with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GameLibs {
    /// Whatever the TS files bring: the TS 3.0 originals are 32-bit Windows only, so Linux runs
    /// Windows Xash3D through Wine.
    Original,
    /// The Rust port's native libraries, copied over `ts/`.
    RustPort(PortSource),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PortSource {
    /// The latest release of the port's repository.
    Release,
    /// A local directory: the port's `dist/`, one platform's `ts/`, or a game directory.
    Dir(PathBuf),
    /// A `.zip` or `.tar.gz` with the same layout.
    Url(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// The Half-Life directory, or where Xash3D goes.
    pub root: PathBuf,
    pub engine: Engine,
    pub ts: TsSource,
    pub libs: GameLibs,
}

/// A finished install, remembered by the launcher.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Installation {
    pub root: PathBuf,
    pub runtime: Runtime,
}

/// How the installed game is started.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Runtime {
    /// Steam/GoldSrc Half-Life.
    GoldSrc,
    Xash(XashBuild),
}

impl Runtime {
    /// The Rust port's `dist/` platform directory name for libraries this runtime loads.
    pub fn port_platform(self) -> &'static str {
        match (self, cfg!(windows)) {
            (Self::GoldSrc, true) => "windows-i686",
            (Self::GoldSrc, false) => "linux-i686",
            (Self::Xash(XashBuild::LinuxAmd64), _) => "linux-x86_64",
            (Self::Xash(XashBuild::WindowsAmd64), _) => "windows-x86_64",
            (Self::Xash(XashBuild::WindowsI386), _) => "windows-i686",
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::GoldSrc => "Half-Life (GoldSrc)".into(),
            Self::Xash(b) if b.needs_wine() => format!("{} via Wine", b.label()),
            Self::Xash(b) => b.label().into(),
        }
    }
}

/// Whether `dir` looks like a Half-Life install.
pub fn is_half_life_dir(dir: &Path) -> bool {
    dir.join("valve").join("liblist.gam").is_file()
}

/// Which game libraries a `ts` directory holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsLibs {
    Original,
    RustPort,
}

impl TsLibs {
    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "TS 3.0 original",
            Self::RustPort => "Rust port",
        }
    }
}

/// What an install folder already contains.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Existing {
    pub valve: bool,
    pub ts: Option<TsLibs>,
    pub xash: Option<XashBuild>,
}

impl Existing {
    pub fn inspect(root: &Path) -> Self {
        if root.as_os_str().is_empty() {
            return Self::default();
        }
        Self { valve: is_half_life_dir(root), ts: ts_libs(&root.join("ts")), xash: xash_build_in(root) }
    }
}

/// The libraries in the game directory `ts`, or `None` when it isn't a TS install.
pub fn ts_libs(ts: &Path) -> Option<TsLibs> {
    if !ts.join("liblist.gam").is_file() || !ts.join("dlls").is_dir() {
        return None;
    }
    let liblist = std::fs::read_to_string(ts.join("liblist.gam")).unwrap_or_default();
    let port = liblist.contains("(Rust)") || ts.join("dlls/ts_amd64.so").is_file() || ts.join("dlls/ts_amd64.dll").is_file();
    Some(if port { TsLibs::RustPort } else { TsLibs::Original })
}

/// The Xash3D FWGS build installed in `root`, judged by its files.
pub fn xash_build_in(root: &Path) -> Option<XashBuild> {
    if root.join("xash3d").is_file() && root.join("libxash.so").is_file() {
        return Some(XashBuild::LinuxAmd64);
    }
    if !root.join("xash3d.exe").is_file() || !root.join("xash.dll").is_file() {
        return None;
    }
    Some(match pe_machine(&root.join("xash.dll")) {
        Some(0x8664) => XashBuild::WindowsAmd64,
        _ => XashBuild::WindowsI386,
    })
}

/// The COFF machine field of a PE file.
fn pe_machine(path: &Path) -> Option<u16> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut b = [0u8; 4];
    f.seek(SeekFrom::Start(0x3C)).ok()?;
    f.read_exact(&mut b).ok()?;
    f.seek(SeekFrom::Start(u32::from_le_bytes(b) as u64 + 4)).ok()?;
    let mut m = [0u8; 2];
    f.read_exact(&mut m).ok()?;
    Some(u16::from_le_bytes(m))
}

impl Plan {
    /// The Xash3D build this plan installs, if any.
    pub fn xash_build(&self) -> Result<Option<XashBuild>> {
        match (&self.engine, &self.libs) {
            (Engine::HalfLife, _) => Ok(None),
            (Engine::Xash { engine: EngineSource::Keep(build), .. }, _) => Ok(Some(*build)),
            (Engine::Xash { .. }, GameLibs::RustPort(_)) => XashBuild::native().map(Some),
            // Keeping a Rust port install needs the native engine, just like installing one.
            (Engine::Xash { .. }, GameLibs::Original)
                if self.ts == TsSource::Keep && ts_libs(&self.root.join("ts")) == Some(TsLibs::RustPort) =>
            {
                XashBuild::native().map(Some)
            }
            (Engine::Xash { .. }, GameLibs::Original) => Ok(Some(XashBuild::WindowsI386)),
        }
    }

    pub fn runtime(&self) -> Result<Runtime> {
        Ok(match self.xash_build()? {
            Some(b) => Runtime::Xash(b),
            None => Runtime::GoldSrc,
        })
    }

    /// The Rust port's `dist/` platform directory name these libraries must come from.
    fn port_platform(&self) -> Result<&'static str> {
        Ok(self.runtime()?.port_platform())
    }

    /// Problems that stop the plan from running, for the wizard to show before it starts.
    pub fn validate(&self) -> Result<()> {
        let empty = |p: &Path| p.as_os_str().is_empty();
        match &self.engine {
            Engine::HalfLife if empty(&self.root) => bail!("choose your Half-Life folder"),
            Engine::HalfLife if !is_half_life_dir(&self.root) => {
                bail!("{} is not a Half-Life install (no valve/liblist.gam)", self.root.display())
            }
            Engine::Xash { .. } if empty(&self.root) => bail!("choose an install folder"),
            Engine::Xash { valve: ValveSource::CopyFrom(hl), .. } if empty(hl) => bail!("choose the Half-Life folder to copy valve from"),
            Engine::Xash { valve: ValveSource::CopyFrom(hl), .. } if !is_half_life_dir(hl) => {
                bail!("{} is not a Half-Life install (no valve/liblist.gam)", hl.display())
            }
            _ => {}
        }
        let existing = Existing::inspect(&self.root);
        if let Engine::Xash { engine: EngineSource::Keep(_), .. } = self.engine
            && existing.xash.is_none()
        {
            bail!("{} has no Xash3D FWGS engine to keep", self.root.display());
        }
        if let Engine::Xash { valve: ValveSource::Keep, .. } = self.engine
            && !existing.valve
        {
            bail!("{} has no valve directory to keep", self.root.display());
        }
        // The original TS libraries are 32-bit Windows DLLs: a kept 64-bit or Linux engine can't load them.
        let libs_after = match (&self.libs, &self.ts) {
            (GameLibs::RustPort(_), _) => Some(TsLibs::RustPort),
            (GameLibs::Original, TsSource::Keep) => existing.ts,
            (GameLibs::Original, _) => Some(TsLibs::Original),
        };
        if let Engine::Xash { engine: EngineSource::Keep(build), .. } = self.engine
            && libs_after == Some(TsLibs::Original)
            && build != XashBuild::WindowsI386
        {
            bail!(
                "the original TS 3.0 DLLs need the 32-bit Windows engine, but this folder has {}: download the engine instead",
                build.label()
            );
        }
        match &self.ts {
            TsSource::Keep if existing.ts.is_none() => bail!("{} has no ts directory to keep", self.root.display()),
            TsSource::Local(p) if empty(p) => bail!("choose the TS installer or a folder with a ts directory"),
            TsSource::Local(p) if !p.is_file() && sources::find_ts_dir(p).is_none() => {
                bail!("{} is neither the TS installer nor a folder with a ts directory", p.display())
            }
            _ => {}
        }
        match &self.libs {
            GameLibs::RustPort(PortSource::Dir(d)) if empty(d) => bail!("choose the Rust port folder"),
            GameLibs::RustPort(PortSource::Dir(d)) => {
                let platform = self.port_platform()?;
                if sources::find_port_dir(d, platform).is_none() {
                    bail!("{} has no Rust port libraries for {platform}", d.display());
                }
            }
            GameLibs::RustPort(PortSource::Url(u)) if !u.starts_with("https://") && !u.starts_with("http://") => {
                bail!("enter an http(s) URL for the Rust port archive")
            }
            _ => {}
        }
        self.xash_build()?;
        Ok(())
    }

    /// Titles of the steps [`Plan::run`] will take, in order.
    pub fn steps(&self) -> Vec<String> {
        let mut steps = Vec::new();
        if let Engine::Xash { engine, valve } = &self.engine {
            if let (EngineSource::Download, Ok(Some(build))) = (engine, self.xash_build()) {
                steps.push(format!("Install {}", build.label()));
            }
            match valve {
                ValveSource::Keep => {}
                ValveSource::XashTs => steps.push("Install Half-Life data from xash3d-ts".into()),
                ValveSource::CopyFrom(p) if p.as_os_str().is_empty() => steps.push("Copy Half-Life data from a local install".into()),
                ValveSource::CopyFrom(p) => steps.push(format!("Copy Half-Life data from {}", p.display())),
            }
        }
        match &self.ts {
            TsSource::Keep => {}
            TsSource::Download => steps.push("Download and install The Specialists 3.0".into()),
            TsSource::Local(p) => steps.push(format!("Install The Specialists 3.0 from {}", p.display())),
        }
        if let GameLibs::RustPort(_) = self.libs {
            steps.push(match self.libs {
                GameLibs::RustPort(PortSource::Release) => "Download and install the latest Rust port release".into(),
                _ => "Install the Rust port libraries".into(),
            });
        }
        steps
    }

    pub fn run(&self, progress: &Progress) -> Result<Installation> {
        self.validate()?;
        std::fs::create_dir_all(&self.root).with_context(|| format!("create {}", self.root.display()))?;
        let titles = self.steps();
        let mut step = 0;
        let mut next = |progress: &Progress| {
            progress.step(step, titles.len(), titles[step].clone());
            step += 1;
        };

        if let Engine::Xash { engine, valve } = &self.engine {
            if let (EngineSource::Download, Some(build)) = (engine, self.xash_build()?) {
                next(progress);
                sources::install_fwgs(build, &self.root, progress)?;
            }
            match valve {
                ValveSource::Keep => {}
                ValveSource::XashTs => {
                    next(progress);
                    sources::install_xash_ts_valve(&self.root, progress)?
                }
                ValveSource::CopyFrom(hl) => {
                    next(progress);
                    sources::copy_valve(hl, &self.root, progress)?
                }
            }
        }

        match &self.ts {
            TsSource::Keep => {}
            TsSource::Download => {
                next(progress);
                let installer = sources::download_ts_installer(progress)?;
                sources::install_ts_from_installer(&installer, &self.root, progress)?;
            }
            TsSource::Local(p) => {
                next(progress);
                sources::install_ts_local(p, &self.root, progress)?
            }
        }

        if let GameLibs::RustPort(src) = &self.libs {
            next(progress);
            let platform = self.port_platform()?;
            let dir = match src {
                PortSource::Release => sources::fetch_port_release(platform, progress)?,
                PortSource::Dir(d) => d.clone(),
                PortSource::Url(u) => sources::fetch_port_archive(u, progress)?,
            };
            install_port(&dir, platform, &self.root.join("ts"), progress)?;
        }

        if titles.is_empty() {
            progress.log("nothing to install: using the existing files");
        }
        progress.log("install complete");
        Ok(Installation { root: self.root.clone(), runtime: self.runtime()? })
    }
}

/// TS 3.0 files the Rust port archives replace, relative to `ts`.
const PORT_REPLACES: &[&str] = &["liblist.gam", "delta.lst", "dlls/mp.dll", "dlls/ts_i386.so", "cl_dlls/client.dll"];
/// Files only the Rust port brings, removed when going back to the TS 3.0 libraries.
const PORT_ONLY: &[&str] = &[
    "gameinfo.txt",
    "dlls/ts.so",
    "dlls/ts_amd64.so",
    "dlls/ts_amd64.dll",
    "cl_dlls/client.so",
    "cl_dlls/client_amd64.so",
    "cl_dlls/client_amd64.dll",
];
/// Where the TS 3.0 files the port replaces are kept, inside `ts`.
const ORIGINALS_DIR: &str = ".tsinstaller-originals";

/// Copies the Rust port for `platform` from `dir` over `ts`, first keeping the TS 3.0 files it
/// replaces so [`restore_original_libs`] can put them back.
fn install_port(dir: &Path, platform: &str, ts: &Path, progress: &Progress) -> Result<()> {
    let port =
        sources::find_port_dir(dir, platform).with_context(|| format!("{} has no Rust port libraries for {platform}", dir.display()))?;
    let originals = ts.join(ORIGINALS_DIR);
    if ts_libs(ts) == Some(TsLibs::Original) && !originals.exists() {
        for rel in crate::archive::list_files(&port)? {
            if ts.join(&rel).is_file() {
                std::fs::create_dir_all(originals.join(&rel).parent().unwrap_or(&originals))?;
                std::fs::copy(ts.join(&rel), originals.join(&rel)).with_context(|| format!("back up {}", rel.display()))?;
            }
        }
        progress.log(format!("kept the TS 3.0 files the port replaces in {}", originals.display()));
    }
    let n = crate::archive::copy_dir(&port, ts, progress)?;
    progress.log(format!("{n} Rust port files copied from {}", port.display()));
    Ok(())
}

/// Puts the TS 3.0 libraries back in `root/ts`: from the copies kept when the port was
/// installed, or else from the TS installer.
fn restore_original_libs(root: &Path, progress: &Progress) -> Result<()> {
    let ts = root.join("ts");
    let originals = ts.join(ORIGINALS_DIR);
    if originals.is_dir() {
        let n = crate::archive::copy_dir(&originals, &ts, progress)?;
        progress.log(format!("{n} TS 3.0 files restored from {}", originals.display()));
    } else {
        let installer = sources::download_ts_installer(progress)?;
        let n = sources::restore_ts_files(&installer, root, PORT_REPLACES, progress)?;
        progress.log(format!("{n} TS 3.0 files restored from {}", installer.display()));
    }
    for f in PORT_ONLY {
        let _ = std::fs::remove_file(ts.join(f));
    }
    Ok(())
}

/// The game libraries an installation can switch between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibChoice {
    /// The TS 3.0 originals: 32-bit Windows, or Linux for a Steam server.
    Original,
    /// The Rust port's 32-bit build, for Half-Life or 32-bit Xash3D.
    RustI686,
    /// The Rust port's 64-bit build, for native 64-bit Xash3D.
    RustAmd64,
}

impl LibChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "TS 3.0 original",
            Self::RustI686 => "Rust port (i686)",
            Self::RustAmd64 => "Rust port (amd64)",
        }
    }

    /// What `install` runs now, or `None` when it has no TS game directory.
    pub fn current(install: &Installation) -> Option<Self> {
        Some(match ts_libs(&install.root.join("ts"))? {
            TsLibs::Original => Self::Original,
            TsLibs::RustPort if matches!(install.runtime, Runtime::Xash(b) if b != XashBuild::WindowsI386) => Self::RustAmd64,
            TsLibs::RustPort => Self::RustI686,
        })
    }

    /// The choices `runtime` can switch to: Half-Life only loads 32-bit libraries.
    pub fn available(runtime: Runtime) -> Vec<Self> {
        match runtime {
            Runtime::GoldSrc => vec![Self::Original, Self::RustI686],
            Runtime::Xash(_) if XashBuild::native().is_err() => vec![Self::Original, Self::RustI686],
            Runtime::Xash(_) => vec![Self::Original, Self::RustI686, Self::RustAmd64],
        }
    }

    /// The runtime that loads these libraries, for an install that runs `runtime` now. Half-Life
    /// stays as it is; Xash3D switches between the 32-bit Windows build and the native one.
    pub fn runtime(self, runtime: Runtime) -> Result<Runtime> {
        Ok(match (self, runtime) {
            (Self::RustAmd64, Runtime::GoldSrc) => bail!("Half-Life only loads 32-bit game libraries"),
            (_, Runtime::GoldSrc) => Runtime::GoldSrc,
            (Self::RustAmd64, Runtime::Xash(_)) => Runtime::Xash(XashBuild::native()?),
            (_, Runtime::Xash(_)) => Runtime::Xash(XashBuild::WindowsI386),
        })
    }
}

/// Switches `install` to the game libraries `choice`, downloading the Xash3D build they need.
pub fn switch_libs(install: &Installation, choice: LibChoice, progress: &Progress) -> Result<Installation> {
    let root = &install.root;
    let runtime = choice.runtime(install.runtime)?;
    // The Windows builds share file names, so a changed engine is always downloaded.
    let new_engine = match runtime {
        Runtime::Xash(b) if runtime != install.runtime => Some(b),
        _ => None,
    };
    let mut titles = Vec::new();
    if let Some(b) = new_engine {
        titles.push(format!("Install {}", b.label()));
    }
    titles.push(match choice {
        LibChoice::Original => "Restore the TS 3.0 libraries".to_owned(),
        _ => format!("Download and install the latest Rust port release ({})", runtime.port_platform()),
    });
    let total = titles.len();
    let mut titles = titles.into_iter().enumerate();
    let mut next = || {
        if let Some((i, title)) = titles.next() {
            progress.step(i, total, title);
        }
    };

    if let Some(b) = new_engine {
        next();
        sources::install_fwgs(b, root, progress)?;
    }
    next();
    match choice {
        LibChoice::Original => restore_original_libs(root, progress)?,
        _ => {
            let platform = runtime.port_platform();
            let dir = sources::fetch_port_release(platform, progress)?;
            install_port(&dir, platform, &root.join("ts"), progress)?;
        }
    }
    progress.log(format!("now running {} on {}", choice.label(), runtime.label()));
    Ok(Installation { root: root.clone(), runtime })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(engine: Engine, libs: GameLibs) -> Plan {
        Plan { root: "/tmp/x".into(), engine, ts: TsSource::Download, libs }
    }

    #[test]
    fn original_libs_use_32_bit_windows_xash() {
        let p = plan(Engine::Xash { engine: EngineSource::Download, valve: ValveSource::XashTs }, GameLibs::Original);
        assert_eq!(p.xash_build().unwrap(), Some(XashBuild::WindowsI386));
        assert_eq!(p.steps().len(), 3);
    }

    #[test]
    fn rust_port_uses_native_xash() {
        let p = plan(
            Engine::Xash { engine: EngineSource::Download, valve: ValveSource::XashTs },
            GameLibs::RustPort(PortSource::Url("https://x/y.zip".into())),
        );
        assert_eq!(p.xash_build().unwrap(), XashBuild::native().ok());
        assert_eq!(p.port_platform().unwrap(), if cfg!(windows) { "windows-x86_64" } else { "linux-x86_64" });
        assert_eq!(p.steps().len(), 4);
    }

    #[test]
    fn keeping_everything_has_no_steps() {
        let mut p = plan(Engine::HalfLife, GameLibs::Original);
        p.ts = TsSource::Keep;
        assert!(p.steps().is_empty());
        let p = Plan {
            root: "/tmp/x".into(),
            engine: Engine::Xash { engine: EngineSource::Keep(XashBuild::WindowsI386), valve: ValveSource::Keep },
            ts: TsSource::Keep,
            libs: GameLibs::Original,
        };
        assert!(p.steps().is_empty());
        assert_eq!(p.runtime().unwrap(), Runtime::Xash(XashBuild::WindowsI386));
    }

    #[test]
    fn lib_choices_pick_the_engine() {
        assert_eq!(LibChoice::RustI686.runtime(Runtime::GoldSrc).unwrap(), Runtime::GoldSrc);
        assert!(LibChoice::RustAmd64.runtime(Runtime::GoldSrc).is_err());
        assert!(!LibChoice::available(Runtime::GoldSrc).contains(&LibChoice::RustAmd64));
        let native = Runtime::Xash(XashBuild::native().unwrap());
        assert_eq!(LibChoice::Original.runtime(native).unwrap(), Runtime::Xash(XashBuild::WindowsI386));
        assert_eq!(LibChoice::RustI686.runtime(native).unwrap().port_platform(), "windows-i686");
        assert_eq!(LibChoice::RustAmd64.runtime(Runtime::Xash(XashBuild::WindowsI386)).unwrap(), native);
    }

    #[test]
    fn port_install_keeps_the_originals_to_restore() {
        let tmp = std::env::temp_dir().join(format!("ts-libs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let write = |p: PathBuf, text: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        let ts = tmp.join("root/ts");
        write(ts.join("liblist.gam"), "game \"The Specialists\"");
        write(ts.join("dlls/mp.dll"), "original mp");
        write(ts.join("cl_dlls/client.dll"), "original client");
        let port = tmp.join("port/windows-i686/ts");
        write(port.join("liblist.gam"), "game \"The Specialists (Rust)\"");
        write(port.join("dlls/mp.dll"), "rust mp");
        write(port.join("gameinfo.txt"), "game ts");

        install_port(&tmp.join("port"), "windows-i686", &ts, &Progress::silent()).unwrap();
        assert_eq!(ts_libs(&ts), Some(TsLibs::RustPort));
        assert_eq!(std::fs::read_to_string(ts.join("dlls/mp.dll")).unwrap(), "rust mp");

        restore_original_libs(&tmp.join("root"), &Progress::silent()).unwrap();
        assert_eq!(ts_libs(&ts), Some(TsLibs::Original));
        assert_eq!(std::fs::read_to_string(ts.join("dlls/mp.dll")).unwrap(), "original mp");
        assert_eq!(std::fs::read_to_string(ts.join("cl_dlls/client.dll")).unwrap(), "original client");
        assert!(!ts.join("gameinfo.txt").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn half_life_installs_only_ts() {
        let p = plan(Engine::HalfLife, GameLibs::Original);
        assert_eq!(p.runtime().unwrap(), Runtime::GoldSrc);
        assert_eq!(p.steps().len(), 1);
    }
}
