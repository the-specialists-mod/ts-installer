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
    /// Xash3D FWGS, downloaded into the install directory with a `valve` directory beside it.
    Xash { valve: ValveSource },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValveSource {
    /// The `valve` directory of the xash3d-ts release.
    XashTs,
    /// Copy `valve` from a local Half-Life install.
    CopyFrom(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TsSource {
    /// `ts-3-final.exe` from this repository's release, or its Internet Archive mirror.
    Download,
    /// A local copy of the installer `.exe`, or a directory holding a `ts` game directory.
    Local(PathBuf),
}

/// Which game libraries (`client` and server DLLs) TS runs with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GameLibs {
    /// The TS 3.0 originals: 32-bit Windows only, so Linux runs Windows Xash3D through Wine.
    Original,
    /// The Rust port's native libraries, copied over `ts/`.
    RustPort(PortSource),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PortSource {
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

impl Plan {
    /// The Xash3D build this plan installs, if any.
    pub fn xash_build(&self) -> Result<Option<XashBuild>> {
        match (&self.engine, &self.libs) {
            (Engine::HalfLife, _) => Ok(None),
            (Engine::Xash { .. }, GameLibs::Original) => Ok(Some(XashBuild::WindowsI386)),
            (Engine::Xash { .. }, GameLibs::RustPort(_)) => XashBuild::native().map(Some),
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
        Ok(match (self.runtime()?, cfg!(windows)) {
            (Runtime::GoldSrc, true) => "windows-i686",
            (Runtime::GoldSrc, false) => "linux-i686",
            (Runtime::Xash(XashBuild::LinuxAmd64), _) => "linux-x86_64",
            (Runtime::Xash(XashBuild::WindowsAmd64), _) => "windows-x86_64",
            (Runtime::Xash(XashBuild::WindowsI386), _) => "windows-i686",
        })
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
            Engine::Xash { valve: ValveSource::CopyFrom(hl) } if empty(hl) => bail!("choose the Half-Life folder to copy valve from"),
            Engine::Xash { valve: ValveSource::CopyFrom(hl) } if !is_half_life_dir(hl) => {
                bail!("{} is not a Half-Life install (no valve/liblist.gam)", hl.display())
            }
            _ => {}
        }
        match &self.ts {
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
        if let Ok(Some(build)) = self.xash_build() {
            steps.push(format!("Install {}", build.label()));
            steps.push(match &self.engine {
                Engine::Xash { valve: ValveSource::XashTs } => "Install Half-Life data from xash3d-ts".into(),
                Engine::Xash { valve: ValveSource::CopyFrom(p) } if p.as_os_str().is_empty() => {
                    "Copy Half-Life data from a local install".into()
                }
                Engine::Xash { valve: ValveSource::CopyFrom(p) } => format!("Copy Half-Life data from {}", p.display()),
                Engine::HalfLife => unreachable!(),
            });
        }
        steps.push(match &self.ts {
            TsSource::Download => "Download and install The Specialists 3.0".into(),
            TsSource::Local(p) => format!("Install The Specialists 3.0 from {}", p.display()),
        });
        if let GameLibs::RustPort(_) = self.libs {
            steps.push("Install the Rust port libraries".into());
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

        if let (Some(build), Engine::Xash { valve }) = (self.xash_build()?, &self.engine) {
            next(progress);
            sources::install_fwgs(build, &self.root, progress)?;
            next(progress);
            match valve {
                ValveSource::XashTs => sources::install_xash_ts_valve(&self.root, progress)?,
                ValveSource::CopyFrom(hl) => sources::copy_valve(hl, &self.root, progress)?,
            }
        }

        next(progress);
        match &self.ts {
            TsSource::Download => {
                let installer = sources::download_ts_installer(progress)?;
                sources::install_ts_from_installer(&installer, &self.root, progress)?;
            }
            TsSource::Local(p) => sources::install_ts_local(p, &self.root, progress)?,
        }

        if let GameLibs::RustPort(src) = &self.libs {
            next(progress);
            let platform = self.port_platform()?;
            let dir = match src {
                PortSource::Dir(d) => d.clone(),
                PortSource::Url(u) => sources::fetch_port_archive(u, progress)?,
            };
            let port = sources::find_port_dir(&dir, platform)
                .with_context(|| format!("{} has no Rust port libraries for {platform}", dir.display()))?;
            let n = crate::archive::copy_dir(&port, &self.root.join("ts"), progress)?;
            progress.log(format!("{n} Rust port files copied from {}", port.display()));
        }

        progress.log("install complete");
        Ok(Installation { root: self.root.clone(), runtime: self.runtime()? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(engine: Engine, libs: GameLibs) -> Plan {
        Plan { root: "/tmp/x".into(), engine, ts: TsSource::Download, libs }
    }

    #[test]
    fn original_libs_use_32_bit_windows_xash() {
        let p = plan(Engine::Xash { valve: ValveSource::XashTs }, GameLibs::Original);
        assert_eq!(p.xash_build().unwrap(), Some(XashBuild::WindowsI386));
        assert_eq!(p.steps().len(), 3);
    }

    #[test]
    fn rust_port_uses_native_xash() {
        let p = plan(Engine::Xash { valve: ValveSource::XashTs }, GameLibs::RustPort(PortSource::Url("https://x/y.zip".into())));
        assert_eq!(p.xash_build().unwrap(), XashBuild::native().ok());
        assert_eq!(p.port_platform().unwrap(), if cfg!(windows) { "windows-x86_64" } else { "linux-x86_64" });
        assert_eq!(p.steps().len(), 4);
    }

    #[test]
    fn half_life_installs_only_ts() {
        let p = plan(Engine::HalfLife, GameLibs::Original);
        assert_eq!(p.runtime().unwrap(), Runtime::GoldSrc);
        assert_eq!(p.steps().len(), 1);
    }
}
