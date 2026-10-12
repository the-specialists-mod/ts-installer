//! The install plan the setup wizard builds, and the code that carries it out.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::progress::Progress;
use crate::sources::{self, Arch, XashBuild};

/// Which engine runs TS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Engine {
    /// An existing Half-Life install: TS goes into its `ts` directory.
    HalfLife,
    /// Xash3D FWGS in the install directory with a `valve` directory beside it. `arch` picks the
    /// native build for the Rust port; the original DLLs always need 32-bit Windows.
    Xash { engine: EngineSource, valve: ValveSource, arch: Arch },
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
    /// A release of the port's repository (the latest when `tag` is `None`): its release build,
    /// or its `-debug` build.
    Release { tag: Option<String>, debug: bool },
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
            (Self::Xash(XashBuild::LinuxI386), _) => "linux-i686",
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
    let port =
        liblist.contains("(Rust)") || ["ts_amd64.so", "mp_amd64.dll", "ts_amd64.dll"].iter().any(|f| ts.join("dlls").join(f).is_file());
    Some(if port { TsLibs::RustPort } else { TsLibs::Original })
}

/// The Xash3D FWGS build installed in `root`, judged by its files.
pub fn xash_build_in(root: &Path) -> Option<XashBuild> {
    if root.join("xash3d").is_file() && root.join("libxash.so").is_file() {
        return Some(match elf_class(&root.join("libxash.so")) {
            Some(1) => XashBuild::LinuxI386,
            _ => XashBuild::LinuxAmd64,
        });
    }
    if !root.join("xash3d.exe").is_file() || !root.join("xash.dll").is_file() {
        return None;
    }
    Some(match pe_machine(&root.join("xash.dll")) {
        Some(0x8664) => XashBuild::WindowsAmd64,
        _ => XashBuild::WindowsI386,
    })
}

/// The ELF class byte: 1 for 32-bit, 2 for 64-bit.
fn elf_class(path: &Path) -> Option<u8> {
    use std::io::Read;
    let mut b = [0u8; 5];
    std::fs::File::open(path).ok()?.read_exact(&mut b).ok()?;
    (b[..4] == *b"\x7fELF").then_some(b[4])
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
            (Engine::Xash { arch, .. }, GameLibs::RustPort(_)) => XashBuild::native_for(*arch).map(Some),
            // Keeping a Rust port install needs the native engine, just like installing one.
            (Engine::Xash { arch, .. }, GameLibs::Original)
                if self.ts == TsSource::Keep && ts_libs(&self.root.join("ts")) == Some(TsLibs::RustPort) =>
            {
                XashBuild::native_for(*arch).map(Some)
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
        if let Engine::Xash { engine, valve, .. } = &self.engine {
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
            steps.push(match &self.libs {
                GameLibs::RustPort(PortSource::Release { tag, debug }) => port_step(tag.as_deref(), *debug, None),
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

        if let Engine::Xash { engine, valve, .. } = &self.engine {
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
            let (dir, manifest) = match src {
                PortSource::Release { tag, debug } => {
                    let (dir, fetched) = sources::fetch_port_release(platform, tag.as_deref(), *debug, progress)?;
                    (dir, Some(PortManifest::new(fetched, platform, *debug)))
                }
                PortSource::Dir(d) => (d.clone(), None),
                PortSource::Url(u) => (sources::fetch_port_archive(u, progress)?, None),
            };
            install_port(&dir, platform, manifest.as_ref(), &self.root.join("ts"), progress)?;
        }

        if titles.is_empty() {
            progress.log("nothing to install: using the existing files");
        }
        progress.log("install complete");
        Ok(Installation { root: self.root.clone(), runtime: self.runtime()? })
    }
}

/// TS 3.0 files the Rust port archives replace, relative to `ts`.
pub(crate) const PORT_REPLACES: &[&str] = &["liblist.gam", "delta.lst", "dlls/mp.dll", "dlls/ts_i386.so", "cl_dlls/client.dll"];
/// Files only the Rust port brings, removed when going back to the TS 3.0 libraries.
const PORT_ONLY: &[&str] = &[
    PORT_MANIFEST,
    PORT_DEBUG_MARKER,
    "gameinfo.txt",
    "dlls/ts.so",
    "dlls/ts_amd64.so",
    "dlls/mp_amd64.dll",
    // Port releases before v3.0.1 named the 64-bit Windows server library this.
    "dlls/ts_amd64.dll",
    "cl_dlls/client.so",
    "cl_dlls/client_amd64.so",
    "cl_dlls/client_amd64.dll",
];
/// Where the TS 3.0 files the port replaces are kept, inside `ts`.
pub(crate) const ORIGINALS_DIR: &str = ".tsinstaller-originals";
/// Launcher 0.3 marked the port's debug build with this; [`PORT_MANIFEST`] replaced it.
const PORT_DEBUG_MARKER: &str = ".tsinstaller-port-debug";
/// The [`PortManifest`] of the port release in `ts`.
const PORT_MANIFEST: &str = ".tsinstaller-port.json";

/// Which port release `ts` holds, written when it is installed from a release. The debug and
/// release builds name their files alike, so this is the only way to tell them apart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortManifest {
    pub tag: String,
    /// The release archive the files came from.
    pub asset: String,
    /// The archive's SHA-256 as published with the release.
    pub sha256: Option<String>,
    pub platform: String,
    pub debug: bool,
}

impl PortManifest {
    fn new(fetched: sources::PortFetched, platform: &str, debug: bool) -> Self {
        Self { tag: fetched.tag, asset: fetched.asset, sha256: fetched.sha256, platform: platform.to_owned(), debug }
    }
}

/// The port release in the game directory `ts`, when it was installed from one.
pub fn port_manifest(ts: &Path) -> Option<PortManifest> {
    serde_json::from_str(&std::fs::read_to_string(ts.join(PORT_MANIFEST)).ok()?).ok()
}

/// "Download and install …" for a port release.
fn port_step(tag: Option<&str>, debug: bool, platform: Option<&str>) -> String {
    let release = tag.map(|t| format!("Rust port {t}")).unwrap_or_else(|| "the latest Rust port release".into());
    let build = if debug { " debug build" } else { "" };
    match platform {
        Some(p) => format!("Download and install {release}{build} ({p})"),
        None => format!("Download and install {release}{build}"),
    }
}

/// Copies the Rust port for `platform` from `dir` over `ts`, first keeping the TS 3.0 files it
/// replaces so [`restore_original_libs`] can put them back. `manifest` records the release it
/// came from; without one (a local folder or URL) the record is removed.
fn install_port(dir: &Path, platform: &str, manifest: Option<&PortManifest>, ts: &Path, progress: &Progress) -> Result<()> {
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
    if dir.starts_with(sources::unpacked_port_dir()) {
        sources::clear_unpacked_port();
    }
    let _ = std::fs::remove_file(ts.join(PORT_DEBUG_MARKER));
    match manifest {
        Some(m) => std::fs::write(ts.join(PORT_MANIFEST), serde_json::to_string_pretty(m)?)?,
        None => {
            let _ = std::fs::remove_file(ts.join(PORT_MANIFEST));
        }
    }
    Ok(())
}

/// How the installed port's files compare with the release they came from.
#[derive(Debug, Clone, PartialEq)]
pub struct PortCheck {
    pub manifest: PortManifest,
    /// Files whose SHA-256 matches the release archive's copy.
    pub matching: usize,
    /// Files that differ from the release, relative to `ts`.
    pub different: Vec<String>,
    /// Files the release has that `ts` lacks.
    pub missing: Vec<String>,
}

impl PortCheck {
    pub fn ok(&self) -> bool {
        self.different.is_empty() && self.missing.is_empty()
    }
}

/// Checks the port files in `root/ts` against the release recorded in its manifest: downloads the
/// release archive (verified against the SHA-256 GitHub publishes for it) and compares the SHA-256
/// of every file in it with the installed copy.
pub fn validate_port(root: &Path, progress: &Progress) -> Result<PortCheck> {
    let ts = root.join("ts");
    let manifest = port_manifest(&ts).context(
        "this install doesn't record which Rust port release it has (it predates launcher 0.5 or came from a folder or URL): pick a version to reinstall it",
    )?;
    progress.step(0, 2, format!("Download {}", manifest.asset));
    let release = sources::port_release(Some(&manifest.tag))?;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == manifest.asset)
        .with_context(|| format!("release {} no longer has {}", manifest.tag, manifest.asset))?;
    let dir = sources::fetch_port_asset(asset, progress)?;
    let port = sources::find_port_dir(&dir, &manifest.platform).with_context(|| format!("{} has no ts folder", manifest.asset))?;
    progress.step(1, 2, "Compare SHA-256 of each file");
    let mut check = PortCheck { manifest, matching: 0, different: Vec::new(), missing: Vec::new() };
    for rel in crate::archive::list_files(&port)? {
        let name = rel.to_string_lossy().replace('\\', "/");
        let installed = ts.join(&rel);
        if !installed.is_file() {
            progress.log(format!("missing: {name}"));
            check.missing.push(name);
        } else if crate::net::file_sha256(&installed)? == crate::net::file_sha256(&port.join(&rel))? {
            check.matching += 1;
        } else {
            progress.log(format!("differs: {name}"));
            check.different.push(name);
        }
    }
    sources::clear_unpacked_port();
    progress.log(format!("{} files match {}", check.matching, check.manifest.asset));
    Ok(check)
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
    /// [`LibChoice::RustI686`] built with debug info and assertions.
    RustI686Debug,
    /// [`LibChoice::RustAmd64`] built with debug info and assertions.
    RustAmd64Debug,
}

impl LibChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "TS 3.0 original",
            Self::RustI686 => "Rust port (i686)",
            Self::RustAmd64 => "Rust port (amd64)",
            Self::RustI686Debug => "Rust port (i686, debug)",
            Self::RustAmd64Debug => "Rust port (amd64, debug)",
        }
    }

    fn amd64(self) -> bool {
        matches!(self, Self::RustAmd64 | Self::RustAmd64Debug)
    }

    fn debug(self) -> bool {
        matches!(self, Self::RustI686Debug | Self::RustAmd64Debug)
    }

    /// What `install` runs now, or `None` when it has no TS game directory.
    pub fn current(install: &Installation) -> Option<Self> {
        let ts = install.root.join("ts");
        if ts_libs(&ts)? == TsLibs::Original {
            return Some(Self::Original);
        }
        let amd64 = matches!(install.runtime, Runtime::Xash(b) if b.is_64bit());
        let debug = port_manifest(&ts).map(|m| m.debug).unwrap_or_else(|| ts.join(PORT_DEBUG_MARKER).exists());
        Some(match (amd64, debug) {
            (false, false) => Self::RustI686,
            (true, false) => Self::RustAmd64,
            (false, true) => Self::RustI686Debug,
            (true, true) => Self::RustAmd64Debug,
        })
    }

    /// The choices `runtime` can switch to: Half-Life only loads 32-bit libraries.
    pub fn available(runtime: Runtime) -> Vec<Self> {
        match runtime {
            Runtime::GoldSrc => vec![Self::Original, Self::RustI686, Self::RustI686Debug],
            Runtime::Xash(_) if XashBuild::native().is_err() => vec![Self::Original, Self::RustI686, Self::RustI686Debug],
            Runtime::Xash(_) => vec![Self::Original, Self::RustI686, Self::RustAmd64, Self::RustI686Debug, Self::RustAmd64Debug],
        }
    }

    /// The runtime that loads these libraries, for an install that runs `runtime` now. Half-Life
    /// stays as it is; Xash3D uses 32-bit Windows for the originals and the native 32- or 64-bit
    /// build for the Rust port.
    pub fn runtime(self, runtime: Runtime) -> Result<Runtime> {
        Ok(match runtime {
            Runtime::GoldSrc if self.amd64() => bail!("Half-Life only loads 32-bit game libraries"),
            Runtime::GoldSrc => Runtime::GoldSrc,
            Runtime::Xash(_) if self.amd64() => Runtime::Xash(XashBuild::native()?),
            Runtime::Xash(_) if self == Self::Original => Runtime::Xash(XashBuild::WindowsI386),
            Runtime::Xash(_) => Runtime::Xash(XashBuild::native_for(Arch::I386)?),
        })
    }
}

/// Switches `install` to the game libraries `choice`, downloading the Xash3D build they need.
/// `tag` picks the port release (the latest when `None`); it also reinstalls the same choice.
pub fn switch_libs(install: &Installation, choice: LibChoice, tag: Option<&str>, progress: &Progress) -> Result<Installation> {
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
        _ => port_step(tag, choice.debug(), Some(runtime.port_platform())),
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
            let (dir, fetched) = sources::fetch_port_release(platform, tag, choice.debug(), progress)?;
            let manifest = PortManifest::new(fetched, platform, choice.debug());
            install_port(&dir, platform, Some(&manifest), &root.join("ts"), progress)?;
            progress.log(format!("installed Rust port {}", manifest.tag));
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
        let p = plan(Engine::Xash { engine: EngineSource::Download, valve: ValveSource::XashTs, arch: Arch::Amd64 }, GameLibs::Original);
        assert_eq!(p.xash_build().unwrap(), Some(XashBuild::WindowsI386));
        assert_eq!(p.steps().len(), 3);
    }

    #[test]
    fn rust_port_uses_native_xash() {
        let p = plan(
            Engine::Xash { engine: EngineSource::Download, valve: ValveSource::XashTs, arch: Arch::Amd64 },
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
            engine: Engine::Xash { engine: EngineSource::Keep(XashBuild::WindowsI386), valve: ValveSource::Keep, arch: Arch::Amd64 },
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
        assert!(LibChoice::RustAmd64Debug.runtime(Runtime::GoldSrc).is_err());
        let native = Runtime::Xash(XashBuild::native().unwrap());
        assert_eq!(LibChoice::Original.runtime(native).unwrap(), Runtime::Xash(XashBuild::WindowsI386));
        let i686 = if cfg!(windows) { "windows-i686" } else { "linux-i686" };
        assert_eq!(LibChoice::RustI686.runtime(native).unwrap().port_platform(), i686);
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

        let manifest = PortManifest {
            tag: "v1.0.0".into(),
            asset: "the-specialists-rs-v1.0.0-windows-i686-debug.zip".into(),
            sha256: None,
            platform: "windows-i686".into(),
            debug: true,
        };
        install_port(&tmp.join("port"), "windows-i686", Some(&manifest), &ts, &Progress::silent()).unwrap();
        assert_eq!(port_manifest(&ts), Some(manifest));
        assert_eq!(ts_libs(&ts), Some(TsLibs::RustPort));
        let install = Installation { root: tmp.join("root"), runtime: Runtime::Xash(XashBuild::WindowsI386) };
        assert_eq!(LibChoice::current(&install), Some(LibChoice::RustI686Debug));
        assert_eq!(std::fs::read_to_string(ts.join("dlls/mp.dll")).unwrap(), "rust mp");

        restore_original_libs(&tmp.join("root"), &Progress::silent()).unwrap();
        assert_eq!(ts_libs(&ts), Some(TsLibs::Original));
        assert_eq!(std::fs::read_to_string(ts.join("dlls/mp.dll")).unwrap(), "original mp");
        assert_eq!(std::fs::read_to_string(ts.join("cl_dlls/client.dll")).unwrap(), "original client");
        assert!(!ts.join("gameinfo.txt").exists());
        assert_eq!(LibChoice::current(&install), Some(LibChoice::Original));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rust_port_can_use_32_bit_xash() {
        let p = plan(
            Engine::Xash { engine: EngineSource::Download, valve: ValveSource::XashTs, arch: Arch::I386 },
            GameLibs::RustPort(PortSource::Release { tag: None, debug: false }),
        );
        let build = if cfg!(windows) { XashBuild::WindowsI386 } else { XashBuild::LinuxI386 };
        assert_eq!(p.xash_build().unwrap(), Some(build));
        assert_eq!(p.port_platform().unwrap(), if cfg!(windows) { "windows-i686" } else { "linux-i686" });
        // The originals ignore the choice: they only run on 32-bit Windows.
        let p = plan(Engine::Xash { engine: EngineSource::Download, valve: ValveSource::XashTs, arch: Arch::Amd64 }, GameLibs::Original);
        assert_eq!(p.xash_build().unwrap(), Some(XashBuild::WindowsI386));
    }

    #[test]
    fn half_life_installs_only_ts() {
        let p = plan(Engine::HalfLife, GameLibs::Original);
        assert_eq!(p.runtime().unwrap(), Runtime::GoldSrc);
        assert_eq!(p.steps().len(), 1);
    }
}
