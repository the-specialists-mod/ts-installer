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

/// Delays before each retry of a request whose connection failed.
const RETRY_DELAYS: [Duration; 2] = if cfg!(test) { [Duration::ZERO; 2] } else { [Duration::from_secs(1), Duration::from_secs(4)] };

/// Sends the request `make` builds, retrying connection-level failures (refused, reset, timed out,
/// Windows running out of sockets) a couple of times: they are often transient. `progress`, when
/// given, hears about retries and can cancel between them.
fn call(
    url: &str,
    progress: Option<&Progress>,
    make: impl Fn() -> ureq::RequestBuilder<ureq::typestate::WithoutBody>,
) -> Result<ureq::http::Response<ureq::Body>> {
    let mut delays = RETRY_DELAYS.iter();
    loop {
        let e = match make().call() {
            Ok(res) => return Ok(res),
            Err(e) => e,
        };
        let retryable =
            matches!(e, ureq::Error::Io(_) | ureq::Error::Timeout(_) | ureq::Error::HostNotFound | ureq::Error::ConnectionFailed);
        match delays.next() {
            Some(delay) if retryable => {
                log::info!("GET {url}: {e}; retrying in {}s", delay.as_secs());
                if let Some(p) = progress {
                    p.log(format!("GET {url}: {e}; retrying in {}s", delay.as_secs()));
                }
                std::thread::sleep(*delay);
                if let Some(p) = progress {
                    p.check_cancelled()?;
                }
            }
            _ => {
                return Err(match hint(&e) {
                    Some(hint) => anyhow::anyhow!("GET {url}: {e}\n{hint}"),
                    None => anyhow::Error::new(e).context(format!("GET {url}")),
                });
            }
        }
    }
}

/// Advice for socket errors that come from the local machine rather than the server.
fn hint(e: &ureq::Error) -> Option<&'static str> {
    let ureq::Error::Io(io) = e else { return None };
    // Winsock codes (WSAENOBUFS, WSAEADDRINUSE, WSAEACCES); no Unix errno is this large.
    match io.raw_os_error()? {
        10055 | 10048 => Some(
            "Windows ran out of network sockets or ports. This usually comes from another program \
             (VPN, antivirus or firewall, \"network accelerator\" software, or an app holding many connections). \
             Restart the PC and try again; if it keeps happening, run `netsh winsock reset` in an administrator \
             Command Prompt and restart.",
        ),
        10013 => Some(
            "Windows refused the connection before it left this PC. A firewall, antivirus or VPN is likely \
             blocking the launcher: allow it there and try again.",
        ),
        _ => None,
    }
}

/// Fetches JSON from the GitHub API.
pub fn get_json(url: &str) -> Result<serde_json::Value> {
    let mut res =
        call(url, None, || agent().get(url).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28"))?;
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
    let mut res = call(url, Some(progress), || {
        let req = agent().get(url);
        if have > 0 { req.header("Range", format!("bytes={have}-")) } else { req }
    })?;
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
/// A release asset as the GitHub API describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct GithubAsset {
    pub url: String,
    /// The SHA-256 GitHub computed when the asset was uploaded, in lowercase hex.
    pub sha256: Option<String>,
    pub size: u64,
}

pub fn github_asset(repo: &str, tag: &str, name: &str) -> Result<GithubAsset> {
    let release = get_json(&format!("https://api.github.com/repos/{repo}/releases/tags/{tag}"))?;
    let a = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"] == name)
        .with_context(|| format!("{repo} release {tag} has no asset {name}"))?;
    Ok(GithubAsset {
        url: a["browser_download_url"].as_str().context("asset without a download URL")?.to_owned(),
        sha256: a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_lowercase),
        size: a["size"].as_u64().unwrap_or(0),
    })
}

/// Whether the file at `path` is `asset`: by SHA-256 when the release publishes one, else by size.
pub fn matches_asset(path: &Path, asset: &GithubAsset) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    Ok(match &asset.sha256 {
        Some(want) => &file_sha256(path)? == want,
        None => asset.size > 0 && fs::metadata(path)?.len() == asset.size,
    })
}

/// Fetches release asset `name` of `repo` into the cache at `dest`. A cached copy that still
/// matches the release (its SHA-256) is used as is; a stale or damaged one is downloaded again.
/// When GitHub can't be reached, a cached copy is used without the check.
pub fn cached_github_asset(repo: &str, tag: &str, name: &str, dest: &Path, progress: &Progress) -> Result<()> {
    let asset = match github_asset(repo, tag, name) {
        Ok(a) => a,
        Err(e) if dest.is_file() => {
            progress.log(format!("couldn't check {name} with GitHub ({e:#}); using the cached copy unchecked"));
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    if dest.is_file() {
        if matches_asset(dest, &asset)? {
            progress.log(format!("using cached {} (matches the release)", dest.display()));
            return Ok(());
        }
        progress.log(format!("cached {name} differs from the release (rebuilt or damaged), downloading it again"));
        fs::remove_file(dest)?;
    }
    download(&asset.url, dest, None, progress)?;
    if !matches_asset(dest, &asset)? && asset.sha256.is_some() {
        let _ = fs::remove_file(dest);
        bail!("{name}: the download doesn't match the SHA-256 published with the release");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::net::TcpListener;

    #[test]
    fn connection_failures_are_retried() {
        // A port that was just free: connecting to it is refused.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}/");
        let tries = Cell::new(0);
        let err = call(&url, None, || {
            tries.set(tries.get() + 1);
            agent().get(&url)
        })
        .unwrap_err();
        assert_eq!(tries.get(), 1 + RETRY_DELAYS.len());
        assert!(format!("{err:#}").starts_with(&format!("GET {url}")), "{err:#}");
    }

    #[test]
    fn windows_socket_exhaustion_gets_a_hint() {
        let e = ureq::Error::Io(io::Error::from_raw_os_error(10055));
        assert!(hint(&e).unwrap().contains("netsh winsock reset"));
        assert!(hint(&ureq::Error::Io(io::Error::from(io::ErrorKind::ConnectionRefused))).is_none());
        assert!(hint(&ureq::Error::HostNotFound).is_none());
    }
}
