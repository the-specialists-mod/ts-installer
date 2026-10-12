//! Updating the launcher itself: checking this repository's latest release for a newer version
//! and replacing the running binary with its build for this platform.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::net;
use crate::progress::Progress;

pub const REPO: &str = "the-specialists-mod/tsmod-launcher";
/// Set to skip the check on start (any non-empty value).
pub const DISABLE_ENV: &str = "TSINSTALLER_NO_UPDATE_CHECK";

/// A newer release with a build for this platform.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    /// The release tag, e.g. `v0.4.0`.
    pub tag: String,
    pub url: String,
    /// The release's page, for its notes.
    pub page: String,
}

/// The running launcher's version.
pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Whether the check on start is turned off.
pub fn disabled() -> bool {
    std::env::var_os(DISABLE_ENV).is_some_and(|v| !v.is_empty())
}

/// The ending of this platform's release asset, as the build workflow names them.
fn asset_suffix() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("-linux-x86_64.bin"),
        ("windows", "x86_64") => Some("-windows-x86_64.exe"),
        _ => None,
    }
}

/// `v1.2.3` or `1.2.3` as numbers; anything after the patch number (`-rc1`) is ignored.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim().trim_start_matches('v').splitn(3, '.');
    let mut next = || parts.next()?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok();
    Some((next()?, next()?, next()?))
}

/// Whether release `tag` is newer than version `current`.
pub fn is_newer(tag: &str, current: &str) -> bool {
    matches!((parse_version(tag), parse_version(current)), (Some(t), Some(c)) if t > c)
}

/// The latest release, when it is newer than this launcher and has a build for this platform.
pub fn check() -> Result<Option<Update>> {
    let release = net::get_json(&format!("https://api.github.com/repos/{REPO}/releases/latest"))?;
    let tag = release["tag_name"].as_str().context("the latest release has no tag")?;
    if !is_newer(tag, current()) {
        return Ok(None);
    }
    let Some(suffix) = asset_suffix() else { return Ok(None) };
    let url = release["assets"].as_array().into_iter().flatten().find_map(|a| {
        let name = a["name"].as_str()?;
        name.ends_with(suffix).then(|| a["browser_download_url"].as_str().map(str::to_owned))?
    });
    let Some(url) = url else {
        log::warn!("release {tag} has no *{suffix} build");
        return Ok(None);
    };
    let page = release["html_url"].as_str().unwrap_or_default().to_owned();
    Ok(Some(Update { tag: tag.to_owned(), url, page }))
}

/// The copy of the old binary Windows keeps until the next start: a running `.exe` can be renamed
/// but not overwritten or deleted.
fn old_copy(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_owned();
    name.push(".old");
    exe.with_file_name(name)
}

/// Downloads `update` and puts it in place of the running binary. Returns the binary's path, to
/// start the new version from.
pub fn install(update: &Update, progress: &Progress) -> Result<PathBuf> {
    let exe = dunce::canonicalize(std::env::current_exe().context("find the running launcher")?)?;
    let mut name = exe.file_name().unwrap_or_default().to_owned();
    name.push(".new");
    // Next to the binary, so the final rename stays on one file system.
    let new = exe.with_file_name(name);
    let _ = std::fs::remove_file(&new);
    net::download(&update.url, &new, None, progress)
        .with_context(|| format!("download {} into {}", update.tag, exe.parent().unwrap_or(&exe).display()))?;
    if std::fs::metadata(&new)?.len() < 1 << 20 {
        let _ = std::fs::remove_file(&new);
        bail!("the {} download is too small to be the launcher", update.tag);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
    }
    if cfg!(windows) {
        let old = old_copy(&exe);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old).with_context(|| format!("move {} aside", exe.display()))?;
    }
    std::fs::rename(&new, &exe).with_context(|| format!("replace {}", exe.display()))?;
    progress.log(format!("installed {} as {}", update.tag, exe.display()));
    Ok(exe)
}

/// Deletes the binary an update on Windows left behind.
pub fn clean_up() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_copy(&exe));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.4.0", "0.3.0"));
        assert!(is_newer("v0.3.1", "0.3.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(!is_newer("v0.3.0", "0.3.0"));
        assert!(!is_newer("v0.2.9", "0.3.0"));
        assert!(!is_newer("ts-3.0", "0.3.0"));
        assert!(is_newer("v0.4.0-rc1", "0.3.0"));
    }
}
