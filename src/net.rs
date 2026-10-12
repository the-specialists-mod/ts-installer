//! HTTP downloads with progress, caching and checksum verification.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use md5::{Digest, Md5};

use crate::progress::Progress;

/// A desktop browser user agent: some mirrors reject unknown clients.
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into()
}

/// Fetches JSON from the GitHub API.
pub fn get_json(url: &str) -> Result<serde_json::Value> {
    let mut res = agent()
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .with_context(|| format!("GET {url}"))?;
    let status = res.status();
    ensure!(status.is_success(), "GET {url}: HTTP {status}");
    Ok(res.body_mut().read_json()?)
}

/// Directory for downloaded archives, kept between runs so a failed install can resume cheaply.
pub fn cache_dir() -> PathBuf {
    crate::store::home().join("cache")
}

/// Downloads `url` to `dest` unless `dest` already exists (and matches `md5`, when given).
/// Writes to `dest.part` first so an interrupted download is never mistaken for a finished one;
/// a later call resumes the `.part` file when the server supports range requests.
pub fn download(url: &str, dest: &Path, md5: Option<&str>, progress: &Progress) -> Result<()> {
    if dest.exists() {
        match md5 {
            Some(want) if file_md5(dest, progress)? != want => {
                progress.log(format!("{} is corrupt, downloading again", dest.display()));
                fs::remove_file(dest)?;
            }
            _ => {
                progress.log(format!("using cached {}", dest.display()));
                return Ok(());
            }
        }
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let part = dest.with_extension("part");
    let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let mut req = agent().get(url);
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let mut res = req.call().with_context(|| format!("GET {url}"))?;
    let status = res.status();
    if status.as_u16() == 416 {
        // The part file is already complete (or bogus): start over.
        let _ = fs::remove_file(&part);
        return download(url, dest, md5, progress);
    }
    ensure!(status.is_success(), "GET {url}: HTTP {status}");
    let resumed = have > 0 && status.as_u16() == 206;
    if !resumed {
        have = 0;
    }
    progress.log(if resumed { format!("resuming {url} at {}", human(have)) } else { format!("downloading {url}") });
    let length: Option<u64> = res.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
    let total = length.map(|l| l + have);

    let mut hasher = Md5::new();
    if resumed {
        let mut existing = File::open(&part)?;
        io::copy(&mut existing, &mut HashWriter(&mut hasher))?;
    }
    let file = if resumed { fs::OpenOptions::new().append(true).open(&part)? } else { File::create(&part)? };
    let mut out = BufWriter::new(file);
    let mut body = res.body_mut().with_config().limit(u64::MAX).reader();
    let mut buf = vec![0u8; 256 << 10];
    let mut done = have;
    loop {
        progress.check_cancelled()?;
        let n = match body.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e).with_context(|| format!("download {url} (stopped at {}; retry to resume)", human(done))),
        };
        out.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        done += n as u64;
        progress.bytes(done, total);
    }
    out.flush()?;
    drop(out);
    if let Some(total) = total {
        ensure!(done == total, "download {url}: got {done} of {total} bytes; retry to resume");
    }
    if let Some(want) = md5 {
        let got = hex(&hasher.finalize());
        if got != want {
            let _ = fs::remove_file(&part);
            bail!("download {url}: MD5 {got}, expected {want}");
        }
    }
    fs::rename(&part, dest)?;
    Ok(())
}

struct HashWriter<'a>(&'a mut Md5);

impl Write for HashWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn human(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
}

pub fn file_md5(path: &Path, progress: &Progress) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let total = file.metadata()?.len();
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        progress.bytes(done, Some(total));
    }
    Ok(hex(&hasher.finalize()))
}

/// The SHA-256 of `path`, in lowercase hex.
pub fn file_sha256(path: &Path) -> Result<String> {
    use sha2::Sha256;
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Download URL of the release asset `name` in `repo` (`owner/name`) at `tag`.
pub fn github_asset_url(repo: &str, tag: &str, name: &str) -> Result<String> {
    let release = get_json(&format!("https://api.github.com/repos/{repo}/releases/tags/{tag}"))?;
    release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"] == name)
        .and_then(|a| a["browser_download_url"].as_str())
        .map(str::to_owned)
        .with_context(|| format!("{repo} release {tag} has no asset {name}"))
}
