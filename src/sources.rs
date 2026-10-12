//! Where each piece comes from: Xash3D FWGS, the `valve` game data, and the TS 3.0 client.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::net;
use crate::progress::Progress;

pub const FWGS_REPO: &str = "FWGS/xash3d-fwgs";
pub const FWGS_TAG: &str = "continuous";

/// Xash3D builds of TS with the Half-Life `valve` data, for players without Half-Life.
pub const XASH_TS_REPO: &str = "The-Specialists-Archive/xash3d-ts";
pub const XASH_TS_TAG: &str = "Mod-Download";
pub const XASH_TS_ASSET: &str = "release.zip";
/// Directory inside `release.zip` that holds the `valve` data.
pub const XASH_TS_VALVE_PREFIX: &str = "xash/valve";

/// `ts-3-final.exe` as an asset of this repository's `ts-3.0` release. ModDB, the original
/// source, sits behind a Cloudflare browser check that rejects scripted downloads.
pub const TS_RELEASE_URL: &str = "https://github.com/the-specialists-mod/ts-installer/releases/download/ts-3.0/ts-3-final.exe";
/// The same file on the Internet Archive, used when the release download fails.
pub const TS_ARCHIVE_ORG: &str = "https://archive.org/download/ts-3.0/ts-3-final.exe";
pub const TS_INSTALLER_NAME: &str = "ts-3-final.exe";
pub const TS_INSTALLER_MD5: &str = "e0fe976a8d9d59393e66b9f1cb94c256";

/// The Rust port's releases: one `the-specialists-rs-<tag>-<platform>` archive per platform.
pub const PORT_REPO: &str = "the-specialists-mod/the-specialistsmod-rs";

/// A Xash3D FWGS release build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum XashBuild {
    /// 32-bit Windows: loads the original TS 3.0 `client.dll` and `mp.dll`.
    WindowsI386,
    WindowsAmd64,
    LinuxAmd64,
}

impl XashBuild {
    pub fn asset(self) -> &'static str {
        match self {
            Self::WindowsI386 => "xash3d-fwgs-win32-i386.7z",
            Self::WindowsAmd64 => "xash3d-fwgs-win32-amd64.7z",
            Self::LinuxAmd64 => "xash3d-fwgs-linux-amd64.tar.gz",
        }
    }

    pub fn is_windows(self) -> bool {
        matches!(self, Self::WindowsI386 | Self::WindowsAmd64)
    }

    /// Engine executable, relative to the install root.
    pub fn executable(self) -> &'static str {
        if self.is_windows() { "xash3d.exe" } else { "xash3d" }
    }

    /// The native 64-bit build for the machine we are running on.
    pub fn native() -> Result<Self> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => Ok(Self::WindowsAmd64),
            ("linux", "x86_64") => Ok(Self::LinuxAmd64),
            (os, arch) => bail!("no native Xash3D FWGS build is set up for {os}/{arch}"),
        }
    }

    /// Whether running this build here needs Wine.
    pub fn needs_wine(self) -> bool {
        self.is_windows() && !cfg!(windows)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::WindowsI386 => "Xash3D FWGS (Windows, 32-bit)",
            Self::WindowsAmd64 => "Xash3D FWGS (Windows, 64-bit)",
            Self::LinuxAmd64 => "Xash3D FWGS (Linux, 64-bit)",
        }
    }
}

/// Downloads and unpacks `build` into `root`.
pub fn install_fwgs(build: XashBuild, root: &Path, progress: &Progress) -> Result<()> {
    let url = net::github_asset_url(FWGS_REPO, FWGS_TAG, build.asset())?;
    let archive = net::cache_dir().join(build.asset());
    // `continuous` is rebuilt in place, so never trust a cached copy from an older run.
    let _ = std::fs::remove_file(&archive);
    net::download(&url, &archive, None, progress)?;
    progress.log(format!("unpacking {} into {}", build.asset(), root.display()));
    let n = if build.is_windows() {
        crate::archive::extract_7z(&archive, root, progress)?
    } else {
        // The Linux tarball has a single top-level directory.
        crate::archive::extract_tar_gz(&archive, root, 1, progress)?
    };
    progress.log(format!("{n} engine files installed"));
    Ok(())
}

/// Downloads the xash3d-ts release and unpacks its `valve` directory into `root/valve`.
pub fn install_xash_ts_valve(root: &Path, progress: &Progress) -> Result<()> {
    let url = net::github_asset_url(XASH_TS_REPO, XASH_TS_TAG, XASH_TS_ASSET)?;
    let archive = net::cache_dir().join("xash3d-ts-release.zip");
    net::download(&url, &archive, None, progress)?;
    progress.log(format!("unpacking {XASH_TS_VALVE_PREFIX}/ into {}", root.join("valve").display()));
    let n = crate::archive::extract_zip(&archive, XASH_TS_VALVE_PREFIX, &root.join("valve"), progress)?;
    if n == 0 {
        bail!("{XASH_TS_ASSET} has no {XASH_TS_VALVE_PREFIX}/ directory");
    }
    progress.log(format!("{n} valve files installed"));
    Ok(())
}

/// Copies `valve` from a Half-Life install into `root/valve`.
pub fn copy_valve(half_life: &Path, root: &Path, progress: &Progress) -> Result<()> {
    let src = half_life.join("valve");
    if !src.join("liblist.gam").is_file() && !src.join("gameinfo.txt").is_file() {
        bail!("{} is not a Half-Life install (no valve/liblist.gam)", half_life.display());
    }
    if same_path(&src, &root.join("valve")) {
        return Ok(());
    }
    let n = crate::archive::copy_dir(&src, &root.join("valve"), progress)?;
    progress.log(format!("{n} valve files copied from {}", src.display()));
    Ok(())
}

/// Downloads `ts-3-final.exe` from this repository's release, falling back to the Internet
/// Archive, and returns its path. Both copies are checked against the known MD5.
pub fn download_ts_installer(progress: &Progress) -> Result<PathBuf> {
    let dest = net::cache_dir().join(TS_INSTALLER_NAME);
    match net::download(TS_RELEASE_URL, &dest, Some(TS_INSTALLER_MD5), progress) {
        Ok(()) => return Ok(dest),
        Err(e) => progress.log(format!("release download failed ({e:#}), using the Internet Archive mirror")),
    }
    progress.check_cancelled()?;
    net::download(TS_ARCHIVE_ORG, &dest, Some(TS_INSTALLER_MD5), progress)?;
    Ok(dest)
}

/// Unpacks the TS installer into `root` (it holds `ts\...` paths).
pub fn install_ts_from_installer(installer: &Path, root: &Path, progress: &Progress) -> Result<()> {
    let mut inst = crate::clickteam::Installer::open(installer)?;
    let total = inst.total_size();
    progress.log(format!("unpacking {} files from {}", inst.entries.len(), installer.display()));
    let n = inst.extract_all(
        root,
        |_| true,
        |done, _| {
            progress.bytes(done, Some(total));
            progress.check_cancelled()
        },
    )?;
    progress.log(format!("{n} TS files installed"));
    Ok(())
}

/// Unpacks only `files` (paths inside `ts`) from the TS installer into `root`.
pub fn restore_ts_files(installer: &Path, root: &Path, files: &[&str], progress: &Progress) -> Result<usize> {
    let wanted: Vec<PathBuf> = files.iter().map(|f| Path::new("ts").join(f)).collect();
    let mut inst = crate::clickteam::Installer::open(installer)?;
    inst.extract_all(root, |rel| wanted.iter().any(|w| w == rel), |_, _| progress.check_cancelled())
}

/// The `ts` game directory inside `dir`: `dir` itself or `dir/ts`.
pub fn find_ts_dir(dir: &Path) -> Option<PathBuf> {
    [dir.to_path_buf(), dir.join("ts")].into_iter().find(|d| d.join("liblist.gam").is_file() && d.join("dlls").is_dir())
}

/// Installs TS from a local installer `.exe` or an existing `ts` directory.
pub fn install_ts_local(source: &Path, root: &Path, progress: &Progress) -> Result<()> {
    if source.is_file() {
        return install_ts_from_installer(source, root, progress);
    }
    let src = find_ts_dir(source).with_context(|| format!("{} has no TS game directory (ts/liblist.gam)", source.display()))?;
    if same_path(&src, &root.join("ts")) {
        progress.log("TS is already in place");
        return Ok(());
    }
    let n = crate::archive::copy_dir(&src, &root.join("ts"), progress)?;
    progress.log(format!("{n} TS files copied from {}", src.display()));
    Ok(())
}

/// Finds the Rust port's game directory for `platform` (e.g. `linux-x86_64`) in a local
/// directory: a `dist/` tree, a single platform's `ts/`, or the game directory itself.
pub fn find_port_dir(dir: &Path, platform: &str) -> Option<PathBuf> {
    let candidates = [dir.join(platform).join("ts"), dir.join("dist").join(platform).join("ts"), dir.join("ts"), dir.to_path_buf()];
    candidates.into_iter().find(|d| d.join("dlls").is_dir() || d.join("cl_dlls").is_dir())
}

/// Fetches a Rust port archive (`.zip` or `.tar.gz`) from `url` and unpacks it into the cache.
pub fn fetch_port_archive(url: &str, progress: &Progress) -> Result<PathBuf> {
    let name = url.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("port.zip");
    let cache = net::cache_dir().join("port");
    let archive = cache.join(name);
    let _ = std::fs::remove_file(&archive);
    net::download(url, &archive, None, progress)?;
    let dir = cache.join("unpacked");
    let _ = std::fs::remove_dir_all(&dir);
    if name.ends_with(".zip") {
        crate::archive::extract_zip(&archive, "", &dir, progress)?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        crate::archive::extract_tar_gz(&archive, &dir, 0, progress)?;
    } else {
        bail!("{name}: expected a .zip or .tar.gz archive");
    }
    Ok(dir)
}

/// Fetches the Rust port's latest release for `platform` (its `-debug` build when `debug`) and
/// unpacks it into the cache.
pub fn fetch_port_release(platform: &str, debug: bool, progress: &Progress) -> Result<PathBuf> {
    let release = net::get_json(&format!("https://api.github.com/repos/{PORT_REPO}/releases/latest"))?;
    let tag = release["tag_name"].as_str().unwrap_or("latest");
    let build = if debug { format!("{platform}-debug") } else { platform.to_owned() };
    let url = port_asset_url(&release, &build).with_context(|| format!("{PORT_REPO} release {tag} has no {build} archive"))?;
    progress.log(format!("Rust port {tag} for {build}"));
    fetch_port_archive(&url, progress)
}

/// Download URL of the archive for `build`: a platform such as `linux-x86_64`, which skips the
/// `-debug` archives, or a platform with `-debug` appended.
fn port_asset_url(release: &serde_json::Value, build: &str) -> Option<String> {
    let assets = release["assets"].as_array()?;
    assets
        .iter()
        .filter_map(|a| Some((a["name"].as_str()?, a["browser_download_url"].as_str()?)))
        .find(|(name, _)| [".zip", ".tar.gz"].iter().any(|ext| name.ends_with(&format!("-{build}{ext}"))))
        .map(|(_, url)| url.to_owned())
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    match (dunce::canonicalize(a), dunce::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_release_archive_for_the_platform() {
        let assets: Vec<_> = [
            "linux-i686-debug.tar.gz",
            "linux-i686.tar.gz",
            "linux-x86_64-debug.tar.gz",
            "linux-x86_64.tar.gz",
            "windows-x86_64-debug.zip",
            "windows-x86_64.zip",
        ]
        .iter()
        .map(|p| serde_json::json!({"name": format!("the-specialists-rs-v0.1.0-{p}"), "browser_download_url": format!("https://x/{p}")}))
        .collect();
        let release = serde_json::json!({ "assets": assets });
        assert_eq!(port_asset_url(&release, "linux-x86_64").as_deref(), Some("https://x/linux-x86_64.tar.gz"));
        assert_eq!(port_asset_url(&release, "linux-i686").as_deref(), Some("https://x/linux-i686.tar.gz"));
        assert_eq!(port_asset_url(&release, "windows-x86_64").as_deref(), Some("https://x/windows-x86_64.zip"));
        assert_eq!(port_asset_url(&release, "windows-i686"), None);
        assert_eq!(port_asset_url(&release, "linux-x86_64-debug").as_deref(), Some("https://x/linux-x86_64-debug.tar.gz"));
        assert_eq!(port_asset_url(&release, "windows-x86_64-debug").as_deref(), Some("https://x/windows-x86_64-debug.zip"));
    }
}
