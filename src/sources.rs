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

pub const TS_MODDB_PAGE: &str = "https://www.moddb.com/mods/the-specialists/downloads/the-specialists-v30-client-windows";
/// The same file on the Internet Archive, used when ModDB refuses the download (it sits behind
/// a Cloudflare browser check).
pub const TS_ARCHIVE_ORG: &str = "https://archive.org/download/ts-3.0/ts-3-final.exe";
pub const TS_INSTALLER_NAME: &str = "ts-3-final.exe";
pub const TS_INSTALLER_MD5: &str = "e0fe976a8d9d59393e66b9f1cb94c256";

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

/// Downloads `ts-3-final.exe`, from ModDB when it lets us and otherwise from the Internet
/// Archive, and returns its path. Both copies are checked against the known MD5.
pub fn download_ts_installer(progress: &Progress) -> Result<PathBuf> {
    let dest = net::cache_dir().join(TS_INSTALLER_NAME);
    match moddb_file_url(TS_MODDB_PAGE) {
        Ok(url) => match net::download(&url, &dest, Some(TS_INSTALLER_MD5), progress) {
            Ok(()) => return Ok(dest),
            Err(e) => progress.log(format!("ModDB download failed ({e:#}), trying the Internet Archive")),
        },
        Err(e) => progress.log(format!("ModDB unavailable ({e:#}), using the Internet Archive mirror")),
    }
    progress.check_cancelled()?;
    net::download(TS_ARCHIVE_ORG, &dest, Some(TS_INSTALLER_MD5), progress)?;
    Ok(dest)
}

/// Resolves a ModDB download page to the file URL: page → `/downloads/start/<id>` →
/// `/downloads/mirror/<id>/<n>/<hash>`, which redirects to the file.
fn moddb_file_url(page: &str) -> Result<String> {
    let html = net::get_text(page)?;
    if html.contains("challenge-platform") {
        bail!("Cloudflare browser check");
    }
    let start = find_href(&html, "/downloads/start/").context("no download link on the ModDB page")?;
    let html = net::get_text(&absolute("https://www.moddb.com", &start))?;
    let mirror = find_href(&html, "/downloads/mirror/").context("no mirror link on the ModDB start page")?;
    Ok(absolute("https://www.moddb.com", &mirror))
}

fn find_href(html: &str, needle: &str) -> Option<String> {
    let at = html.find(needle)?;
    let start = html[..at].rfind(['"', '\''])? + 1;
    let end = at + html[at..].find(['"', '\''])?;
    Some(html[start..end].replace("&amp;", "&"))
}

fn absolute(base: &str, href: &str) -> String {
    if href.starts_with("http") { href.to_owned() } else { format!("{base}{href}") }
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

pub fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_moddb_links() {
        let html = r#"<a href="/downloads/start/123456" class="button">Download now</a>"#;
        assert_eq!(find_href(html, "/downloads/start/").as_deref(), Some("/downloads/start/123456"));
        let html = r#"<a href="/downloads/mirror/123456/131/abc?x=1&amp;y=2">here</a>"#;
        assert_eq!(find_href(html, "/downloads/mirror/").as_deref(), Some("/downloads/mirror/123456/131/abc?x=1&y=2"));
    }
}
