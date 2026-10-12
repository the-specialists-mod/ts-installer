//! A Steam broker for Xash3D FWGS, so it can join Steam-authenticated GoldSrc servers.
//!
//! FWGS can't use the Steam API itself (GPLv3), so it asks a broker over TCP for the ticket a
//! GoldSrc server wants (Documentation/extensions/steam-broker.md in FWGS; this follows
//! FWGS/steam-broker). The engine uses it when `cl_ticket_generator` is `steam`, connecting to
//! `cl_steam_broker_addr` (127.0.0.1:27420 by default), and falls back to its own ticket when
//! nothing listens there.
//!
//! The launcher runs the broker as a child process (`ts-launcher steam-broker`) next to the game.
//! It ends when the game does: on `sb_terminate`, when the game's connection drops, or when no
//! game connects at all. Steam shows the user as playing Half-Life while it holds a session.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail, ensure};
use steamworks::sys;

/// The command line `start` runs the child with.
pub const COMMAND: &str = "steam-broker";
/// Where FWGS looks for a broker unless `cl_steam_broker_addr` says otherwise.
pub const ADDR: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 27420);
/// What the child prints once it is ready, or once it finds another broker already listening.
const READY: &str = "broker ready";
const IN_USE: &str = "broker in use";
const NOT_RUNNING: i32 = 3;

const HEADER: &[u8; 4] = b"SBRK";
const MAX_PAYLOAD: usize = 4096;
const RESPONSE: &[u8] = b"sb_connect\n";
/// FWGS's limit for a ticket.
const MAX_TICKET: usize = 2048;
const POLL: Duration = Duration::from_millis(100);
/// How long the broker waits for the game to connect before giving up.
const WAIT_FOR_GAME: Duration = Duration::from_secs(120);
const START_TIMEOUT: Duration = Duration::from_secs(5);

/// Starts a broker for a game about to be launched. True when the game should use one: ours is
/// listening, or another one already was.
pub fn start() -> bool {
    match try_start() {
        Ok(ready) => ready,
        Err(e) => {
            log::warn!("Steam broker: {e:#}");
            false
        }
    }
}

fn try_start() -> Result<bool> {
    let log = std::fs::File::create(crate::store::home().join("steam-broker.log")).ok();
    let mut cmd = Command::new(std::env::current_exe().context("find the launcher's own path")?);
    cmd.arg(COMMAND).stdin(Stdio::null()).stdout(Stdio::piped());
    cmd.stderr(log.map(Stdio::from).unwrap_or_else(Stdio::null));
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd.spawn().context("start the Steam broker")?;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx.recv_timeout(START_TIMEOUT).unwrap_or_default();
    let ready = match line.trim() {
        READY => true,
        IN_USE => {
            log::info!("Steam broker: one is already listening on {ADDR}");
            true
        }
        _ => {
            let _ = child.kill();
            false
        }
    };
    // It ends with the game; reap it then.
    std::thread::spawn(move || {
        if let Ok(status) = child.wait()
            && status.code() == Some(NOT_RUNNING)
        {
            log::info!("Steam broker: Steam isn't running");
        }
    });
    Ok(ready)
}

/// `ts-launcher steam-broker`: serves one game, then exits.
pub fn run() -> Result<()> {
    crate::steam::load()?;
    if !unsafe { sys::SteamAPI_IsSteamRunning() } {
        eprintln!("Steam isn't running");
        std::process::exit(NOT_RUNNING);
    }
    let listener = match TcpListener::bind(ADDR) {
        Ok(l) => l,
        Err(e) if e.kind() == ErrorKind::AddrInUse => {
            ready(IN_USE);
            return Ok(());
        }
        Err(e) => return Err(e).with_context(|| format!("listen on {ADDR}")),
    };
    listener.set_nonblocking(true)?;
    eprintln!("listening on {ADDR}");
    ready(READY);

    let mut steam = None;
    let mut deadline = Instant::now() + WAIT_FOR_GAME;
    loop {
        let stream = match listener.accept() {
            Ok((stream, peer)) => {
                eprintln!("game connected from {peer}");
                stream
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                ensure!(Instant::now() < deadline, "no game connected within {}s", WAIT_FOR_GAME.as_secs());
                std::thread::sleep(POLL);
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let result = Session::new(stream)?.run(&mut steam);
        // Steam can't start again in the same process, and its session belongs to the game that
        // just went away.
        if steam.is_some() {
            return result;
        }
        if let Err(e) = result {
            eprintln!("session: {e:#}");
        }
        deadline = Instant::now() + WAIT_FOR_GAME;
    }
}

/// Tells the launcher how it went. Nothing is printed on stdout after this: the launcher stops
/// reading it.
fn ready(line: &str) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

struct Steam {
    client: steamworks::Client,
}

impl Steam {
    fn start(app_id: u32) -> Result<Self> {
        let client = steamworks::Client::init_app(app_id).map_err(|e| anyhow!("Steam: {e}"))?;
        eprintln!("Steam started as app {app_id} for {:?}", client.user().steam_id());
        Ok(Self { client })
    }

    fn user(&self) -> *mut sys::ISteamUser {
        unsafe { sys::SteamAPI_SteamUser_v023() }
    }

    /// The ticket a GoldSrc server checks (InitiateGameConnection, which GoldSrc still uses).
    fn ticket(&self, server: SocketAddrV4, server_id: u64, secure: bool) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; MAX_TICKET];
        let n = unsafe {
            sys::SteamAPI_ISteamUser_InitiateGameConnection_DEPRECATED(
                self.user(),
                buf.as_mut_ptr().cast(),
                buf.len() as i32,
                server_id,
                u32::from(*server.ip()),
                server.port(),
                secure,
            )
        };
        ensure!(n > 0, "Steam gave no ticket for {server}");
        buf.truncate(n as usize);
        Ok(buf)
    }

    fn end_game_connection(&self, server: SocketAddrV4) {
        unsafe { sys::SteamAPI_ISteamUser_TerminateGameConnection_DEPRECATED(self.user(), u32::from(*server.ip()), server.port()) }
    }
}

/// The Steam app a game folder belongs to (as FWGS/steam-broker has it), Half-Life for mods.
fn app_id(gamedir: &str) -> u32 {
    match gamedir.to_ascii_lowercase().as_str() {
        "cstrike" => 10,
        "tfc" => 20,
        "dod" => 30,
        "dmc" => 40,
        "gearbox" => 50,
        "ricochet" => 60,
        "czero" => 80,
        "czeror" => 100,
        "bshift" => 130,
        _ => 70,
    }
}

/// One game's connection.
struct Session {
    stream: TcpStream,
    rx: Vec<u8>,
    active: bool,
    /// The server a ticket was last handed out for, and its challenge.
    joined: Option<(SocketAddrV4, i32)>,
}

impl Session {
    fn new(stream: TcpStream) -> Result<Self> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(POLL))?;
        let _ = stream.set_nodelay(true);
        Ok(Self { stream, rx: Vec::new(), active: false, joined: None })
    }

    fn run(&mut self, steam: &mut Option<Steam>) -> Result<()> {
        let result = self.serve(steam);
        if let (Some((server, _)), Some(s)) = (self.joined.take(), steam.as_ref()) {
            s.end_game_connection(server);
        }
        result
    }

    fn serve(&mut self, steam: &mut Option<Steam>) -> Result<()> {
        let mut buf = [0u8; 4096];
        loop {
            if let Some(s) = steam.as_ref() {
                s.client.run_callbacks();
            }
            match self.stream.read(&mut buf) {
                Ok(0) => {
                    eprintln!("game disconnected");
                    return Ok(());
                }
                Ok(n) => {
                    ensure!(self.rx.len() + n <= 2 * (MAX_PAYLOAD + 6), "too much unread data");
                    self.rx.extend_from_slice(&buf[..n]);
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) => return Err(e.into()),
            }
            while let Some(payload) = take_frame(&mut self.rx)? {
                let text = String::from_utf8(payload).context("command isn't text")?;
                if !self.command(&text, steam)? {
                    eprintln!("game quit");
                    return Ok(());
                }
            }
        }
    }

    /// Handles one command; false once the game has quit.
    fn command(&mut self, text: &str, steam: &mut Option<Steam>) -> Result<bool> {
        let mut args = text.split_ascii_whitespace();
        let cmd = args.next().unwrap_or_default();
        eprintln!("> {text}");
        match cmd {
            "sb_gamedir" => {
                ensure!(!self.active, "session already active");
                let gamedir = args.next().context("sb_gamedir without a game folder")?;
                if steam.is_none() {
                    *steam = Some(Steam::start(app_id(gamedir))?);
                }
                self.active = true;
            }
            "sb_connect" => {
                ensure!(self.active, "sb_connect before sb_gamedir");
                let server: SocketAddrV4 = parse(args.next(), "server address")?;
                let server_id: u64 = parse(args.next(), "server SteamID")?;
                let secure = parse::<i32>(args.next(), "secure")? != 0;
                let challenge: i32 = parse(args.next(), "challenge")?;
                let s = steam.as_ref().context("Steam isn't started")?;
                if let Some((old, _)) = self.joined.take() {
                    s.end_game_connection(old);
                }
                let ticket = s.ticket(server, server_id, secure)?;
                self.joined = Some((server, challenge));
                eprintln!("ticket for {server}: {} bytes", ticket.len());
                let me = s.client.user().steam_id().raw();
                self.stream.write_all(&frame(&connect_response(challenge, me, &ticket))?)?;
            }
            "sb_disconnect" => {
                let server: SocketAddrV4 = parse(args.next(), "server address")?;
                let challenge: i32 = parse(args.next(), "challenge")?;
                if self.joined == Some((server, challenge))
                    && let Some(s) = steam.as_ref()
                {
                    s.end_game_connection(server);
                    self.joined = None;
                }
            }
            "sb_terminate" => return Ok(false),
            _ => bail!("unknown command {cmd:?}"),
        }
        Ok(true)
    }
}

fn parse<T: std::str::FromStr>(arg: Option<&str>, what: &str) -> Result<T> {
    arg.with_context(|| format!("missing {what}"))?.parse().map_err(|_| anyhow!("bad {what}"))
}

/// Takes the first whole frame's payload off `rx`, if one is there.
fn take_frame(rx: &mut Vec<u8>) -> Result<Option<Vec<u8>>> {
    if rx.len() < 6 {
        return Ok(None);
    }
    ensure!(&rx[..4] == HEADER, "not a broker frame");
    let len = u16::from_le_bytes([rx[4], rx[5]]) as usize;
    ensure!(len <= MAX_PAYLOAD, "frame too large ({len} bytes)");
    if rx.len() < 6 + len {
        return Ok(None);
    }
    let payload = rx[6..6 + len].to_vec();
    rx.drain(..6 + len);
    Ok(Some(payload))
}

fn frame(payload: &[u8]) -> Result<Vec<u8>> {
    ensure!(payload.len() <= MAX_PAYLOAD, "reply too large");
    let mut f = Vec::with_capacity(6 + payload.len());
    f.extend_from_slice(HEADER);
    f.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    f.extend_from_slice(payload);
    Ok(f)
}

/// `sb_connect`'s reply: the challenge back, the player's SteamID and the ticket.
fn connect_response(challenge: i32, steam_id: u64, ticket: &[u8]) -> Vec<u8> {
    let mut p = RESPONSE.to_vec();
    p.extend_from_slice(&challenge.to_le_bytes());
    p.extend_from_slice(&steam_id.to_le_bytes());
    p.extend_from_slice(&(ticket.len() as u32).to_le_bytes());
    p.extend_from_slice(ticket);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames() {
        let terminate = frame(b"sb_terminate").unwrap();
        let mut rx = frame(b"sb_gamedir ts").unwrap();
        rx.extend_from_slice(&terminate[..8]);
        assert_eq!(&rx[..6], b"SBRK\x0d\x00");
        assert_eq!(take_frame(&mut rx).unwrap().unwrap(), b"sb_gamedir ts");
        // The second frame isn't all there yet.
        assert_eq!(take_frame(&mut rx).unwrap(), None);
        rx.extend_from_slice(&terminate[8..]);
        assert_eq!(take_frame(&mut rx).unwrap().unwrap(), b"sb_terminate");
        assert!(rx.is_empty());
        assert!(take_frame(&mut b"XXXX\x00\x00".to_vec()).is_err());
    }

    #[test]
    fn connect_reply_layout() {
        let p = connect_response(-2, 76561197960287930, &[1, 2, 3]);
        assert_eq!(&p[..11], b"sb_connect\n");
        assert_eq!(i32::from_le_bytes(p[11..15].try_into().unwrap()), -2);
        assert_eq!(u64::from_le_bytes(p[15..23].try_into().unwrap()), 76561197960287930);
        assert_eq!(u32::from_le_bytes(p[23..27].try_into().unwrap()), 3);
        assert_eq!(&p[27..], [1, 2, 3]);
    }

    #[test]
    fn mods_are_half_life() {
        assert_eq!(app_id("ts"), 70);
        assert_eq!(app_id("CStrike"), 10);
    }
}
