//! Steam's server list through the Steam client (steamworks-rs), when Steam is running.
//!
//! The Steam API library is built into the launcher and only loaded here (see build.rs). While a
//! program uses it, Steam shows the user as playing Half-Life, so the list is asked for by a
//! short-lived child process (`ts-launcher steam-servers`) that exits as soon as it has it. That
//! also keeps a crash inside the Steam API out of the launcher.

use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddrV4;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

mod stubs {
    include!(concat!(env!("OUT_DIR"), "/steam_stubs.rs"));
}

/// The command line `server_list` runs the child with.
pub const COMMAND: &str = "steam-servers";
const HALF_LIFE: u32 = 70;
/// The child's exit code when Steam isn't running (or won't talk to it).
const NOT_RUNNING: i32 = 3;
/// Marks the child's answers: the Steam API prints its own lines on stdout too.
const PREFIX: &str = "server ";
/// How long the child may take, Steam's own refresh included.
const TIMEOUT: Duration = Duration::from_secs(30);
const LIST_WAIT: Duration = Duration::from_secs(20);

#[cfg(target_os = "linux")]
const LIBRARY: (&str, &[u8]) = ("libsteam_api.so", include_bytes!("../assets/steam/libsteam_api.so"));
#[cfg(windows)]
const LIBRARY: (&str, &[u8]) = ("steam_api64.dll", include_bytes!("../assets/steam/steam_api64.dll"));

/// Writes the built-in library to the launcher's directory (in a folder named after its hash, so
/// another launcher version holding its own copy open doesn't get in the way) and points the
/// stubs at it.
fn load() -> Result<()> {
    let (name, bytes) = LIBRARY;
    let hash: String = Sha256::digest(bytes)[..6].iter().map(|b| format!("{b:02x}")).collect();
    let dir: PathBuf = crate::store::home().join("steam").join(hash);
    let path = dir.join(name);
    if std::fs::metadata(&path).map(|m| m.len()).ok() != Some(bytes.len() as u64) {
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let tmp = dir.join(format!("{name}.{}", std::process::id()));
        std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("write {}", path.display()))?;
    }
    let lib = unsafe { libloading::Library::new(&path) }.with_context(|| format!("load {}", path.display()))?;
    for (name, slot) in stubs::EXPORTS.iter().zip(&stubs::SLOTS) {
        if let Ok(f) = unsafe { lib.get::<*const ()>(name.as_bytes()) } {
            slot.store(*f as usize, Ordering::Release);
        }
    }
    // The stubs point into it from now on.
    std::mem::forget(lib);
    Ok(())
}

/// `ts-launcher steam-servers`: prints `server ADDR` for every TS server Steam lists, then exits
/// (with `NOT_RUNNING` if Steam isn't there to ask).
pub fn print_server_list() -> Result<()> {
    load()?;
    let client = match steamworks::Client::init_app(HALF_LIFE) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Steam: {e}");
            std::process::exit(NOT_RUNNING);
        }
    };
    let done = Arc::new(Mutex::new(false));
    let print = |list: Arc<Mutex<steamworks::ServerListRequest>>, server: i32| {
        if let Ok(s) = list.lock().unwrap().get_server_details(server) {
            println!("{PREFIX}{}", SocketAddrV4::new(s.addr, s.query_port));
        }
    };
    let finished = done.clone();
    let callbacks = steamworks::ServerListCallbacks::new(
        Box::new(print),
        // Listed but not answering Steam: the launcher asks it again itself.
        Box::new(print),
        Box::new(move |list, _| {
            let _ = list.lock().unwrap().release();
            *finished.lock().unwrap() = true;
        }),
    );
    let filters = [("gamedir", crate::servers::GAMEDIR)].into_iter().collect();
    client
        .matchmaking_servers()
        .internet_server_list(HALF_LIFE, &filters, callbacks)
        .map_err(|()| anyhow::anyhow!("bad server list filter"))?;
    let start = Instant::now();
    while !*done.lock().unwrap() && start.elapsed() < LIST_WAIT {
        client.run_callbacks();
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

/// Asks the Steam client for TS servers, calling `found` with each address as it comes in.
/// Returns false, having found nothing, when Steam isn't running.
pub fn server_list(found: &mut dyn FnMut(String)) -> Result<bool> {
    let mut cmd = Command::new(std::env::current_exe().context("find the launcher's own path")?);
    cmd.arg(COMMAND).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd.spawn().context("start the Steam query")?;
    let (stdout, mut stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    // Stop it if Steam never finishes.
    let child = Arc::new(Mutex::new(child));
    let (stop, stopped) = mpsc::channel::<()>();
    let watchdog = {
        let child = child.clone();
        std::thread::spawn(move || {
            if stopped.recv_timeout(TIMEOUT) == Err(mpsc::RecvTimeoutError::Timeout) {
                let _ = child.lock().unwrap().kill();
            }
        })
    };
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(addr) = line.strip_prefix(PREFIX) {
            found(addr.trim().to_owned());
        }
    }
    let _ = stop.send(());
    let _ = watchdog.join();
    let status = child.lock().unwrap().wait()?;
    let errors = errors.join().unwrap_or_default();
    match status.code() {
        Some(0) => Ok(true),
        Some(NOT_RUNNING) => {
            log::info!("Steam client: {}", errors.trim());
            Ok(false)
        }
        _ => {
            let last = errors.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output");
            bail!("Steam client query failed ({status}): {last}")
        }
    }
}
