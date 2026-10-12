//! Finding TS servers and asking them about themselves (A2S_INFO / A2S_PLAYER).
//!
//! The UDP master servers GoldSrc and Xash used are gone, so the list comes from this repository's
//! `servers.txt`, Xash3D FWGS's HTTP list, Steam (through the Steam client when it is running,
//! or the Web API with a key), a LAN broadcast and the user's favorites. Every address is then
//! queried directly.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};

/// The game directory servers must report.
pub const GAMEDIR: &str = "ts";
pub const DEFAULT_PORT: u16 = 27015;
/// The curated list, kept in this repository.
pub const CURATED_URL: &str = "https://raw.githubusercontent.com/the-specialists-mod/tsmod-launcher/main/servers.txt";
/// The copy built into the launcher, used with the downloaded one (which may be newer).
const CURATED_BUILTIN: &str = include_str!("../servers.txt");
const FWGS_LIST_URL: &str = "http://xash.su/server-list/v1/servers/";
const STEAM_LIST_URL: &str = "https://api.steampowered.com/IGameServersService/GetServerList/v1/";

const INFO_QUERY: &[u8] = b"\xff\xff\xff\xffTSource Engine Query\x00";
const PLAYER_QUERY: &[u8] = b"\xff\xff\xff\xffU";
/// How long one query waits for each reply.
const TIMEOUT: Duration = Duration::from_millis(1500);
/// How long the LAN broadcast collects replies.
const LAN_WAIT: Duration = Duration::from_millis(1500);
const LAN_PORTS: std::ops::RangeInclusive<u16> = 27015..=27020;
/// Servers queried at the same time.
const WORKERS: usize = 24;

/// Where a server address came from. A server found several ways has several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Favorite,
    Lan,
    List,
    Steam,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Favorite => "Favorite",
            Self::Lan => "LAN",
            Self::List => "List",
            Self::Steam => "Steam",
        }
    }
}

/// What a server says in its A2S_INFO reply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerInfo {
    pub name: String,
    pub map: String,
    pub folder: String,
    pub game: String,
    pub players: u8,
    pub max_players: u8,
    /// Bots among `players` (0 when the server doesn't say).
    pub bots: u8,
    /// `d` dedicated, `l` listen, `p` SourceTV.
    pub server_type: char,
    /// `l` Linux, `w` Windows, `m` macOS.
    pub os: char,
    pub password: bool,
    pub vac: bool,
}

impl ServerInfo {
    pub fn is_ts(&self) -> bool {
        self.folder.eq_ignore_ascii_case(GAMEDIR)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInfo {
    pub name: String,
    pub score: i32,
    /// Seconds connected; negative for bots on some engines.
    pub duration: f32,
}

/// Reads C strings and little-endian numbers from a reply.
struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Option<u8> {
        let b = *self.d.get(self.p)?;
        self.p += 1;
        Some(b)
    }

    fn bytes<const N: usize>(&mut self) -> Option<[u8; N]> {
        let b = self.d.get(self.p..self.p + N)?.try_into().ok()?;
        self.p += N;
        Some(b)
    }

    fn cstr(&mut self) -> Option<String> {
        let end = self.d.get(self.p..)?.iter().position(|&b| b == 0)? + self.p;
        let s = String::from_utf8_lossy(&self.d[self.p..end]).trim().to_owned();
        self.p = end + 1;
        Some(s)
    }
}

/// Parses an A2S_INFO reply: Source style (`I`, which FWGS and newer GoldSrc send) or old GoldSrc
/// (`m`, which tsmod.net's server sends).
pub fn parse_info(d: &[u8]) -> Option<ServerInfo> {
    if d.get(..4)? != b"\xff\xff\xff\xff" {
        return None;
    }
    let mut r = Reader { d, p: 5 };
    let mut info = ServerInfo::default();
    match d[4] {
        b'I' => {
            r.u8()?; // protocol
            (info.name, info.map, info.folder, info.game) = (r.cstr()?, r.cstr()?, r.cstr()?, r.cstr()?);
            r.bytes::<2>()?; // app id
            (info.players, info.max_players) = (r.u8()?, r.u8()?);
            info.bots = r.u8().unwrap_or(0);
            info.server_type = r.u8().map(char::from).unwrap_or('?');
            info.os = r.u8().map(char::from).unwrap_or('?');
            info.password = r.u8().unwrap_or(0) != 0;
            info.vac = r.u8().unwrap_or(0) != 0;
        }
        b'm' => {
            r.cstr()?; // the server's own address
            (info.name, info.map, info.folder, info.game) = (r.cstr()?, r.cstr()?, r.cstr()?, r.cstr()?);
            (info.players, info.max_players) = (r.u8()?, r.u8()?);
            r.u8()?; // protocol
            info.server_type = r.u8().map(char::from).unwrap_or('?');
            info.os = r.u8().map(char::from).unwrap_or('?');
            info.password = r.u8().unwrap_or(0) != 0;
            // A mod server adds its links, version and size before the VAC flag.
            if r.u8() == Some(1) {
                r.cstr();
                r.cstr();
                r.u8();
                r.bytes::<10>();
            }
            info.vac = r.u8().unwrap_or(0) != 0;
            info.bots = r.u8().unwrap_or(0);
        }
        _ => return None,
    }
    Some(info)
}

/// Parses an A2S_PLAYER reply.
pub fn parse_players(d: &[u8]) -> Option<Vec<PlayerInfo>> {
    if d.get(..5)? != b"\xff\xff\xff\xffD" {
        return None;
    }
    let mut r = Reader { d, p: 5 };
    let count = r.u8()?;
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        r.u8()?; // index
        let name = r.cstr()?;
        let score = i32::from_le_bytes(r.bytes()?);
        let duration = f32::from_le_bytes(r.bytes()?);
        out.push(PlayerInfo { name, score, duration });
    }
    Some(out)
}

/// The challenge number from an `A` reply.
fn challenge(d: &[u8]) -> Option<[u8; 4]> {
    (d.len() >= 9 && d.starts_with(b"\xff\xff\xff\xffA")).then(|| d[5..9].try_into().unwrap())
}

/// Sends `query` (answering a challenge once) and returns the reply and the fastest round trip.
fn ask(addr: SocketAddr, query: &[u8], challenge_base: &[u8]) -> std::io::Result<(Vec<u8>, Duration)> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    s.set_read_timeout(Some(TIMEOUT))?;
    let mut buf = [0u8; 4096];
    let mut last_err = None;
    // One resend for a lost packet.
    for _ in 0..2 {
        let start = Instant::now();
        s.send_to(query, addr)?;
        let n = match s.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        };
        let mut ping = start.elapsed();
        if let Some(c) = challenge(&buf[..n]) {
            let mut q = challenge_base.to_vec();
            q.extend_from_slice(&c);
            let start = Instant::now();
            s.send_to(&q, addr)?;
            let n = s.recv(&mut buf)?;
            ping = ping.min(start.elapsed());
            return Ok((buf[..n].to_vec(), ping));
        }
        return Ok((buf[..n].to_vec(), ping));
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::other("no reply")))
}

/// A timeout reads as "no reply"; other errors (unreachable network, refused) keep their text.
fn no_reply(e: std::io::Error) -> anyhow::Error {
    match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => anyhow::anyhow!("no reply"),
        _ => anyhow::anyhow!("no reply ({e})"),
    }
}

/// A2S_INFO and the round trip time.
pub fn query_info(addr: SocketAddr) -> Result<(ServerInfo, Duration)> {
    let (reply, ping) = ask(addr, INFO_QUERY, INFO_QUERY).map_err(no_reply)?;
    let info = parse_info(&reply).context("not a server info reply")?;
    Ok((info, ping))
}

/// A2S_PLAYER: everyone connected, bots included.
pub fn query_players(addr: SocketAddr) -> Result<Vec<PlayerInfo>> {
    let (reply, _) = ask(addr, b"\xff\xff\xff\xffU\xff\xff\xff\xff", PLAYER_QUERY).map_err(no_reply)?;
    parse_players(&reply).context("not a player list reply")
}

/// `host:port` (or a bare host, on the default port) as an IPv4 socket address.
pub fn resolve(addr: &str) -> Result<SocketAddr> {
    let addr = addr.trim();
    let with_port = if addr.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
        addr.to_owned()
    } else {
        format!("{addr}:{DEFAULT_PORT}")
    };
    let mut all = with_port.to_socket_addrs().with_context(|| format!("can't resolve {addr}"))?;
    all.find(SocketAddr::is_ipv4).with_context(|| format!("{addr} has no IPv4 address"))
}

/// Addresses in Xash3D FWGS's list format: `ip|gs host:port` lines, `#` comments.
pub fn parse_list(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter_map(|l| {
            let mut t = l.split_whitespace();
            match (t.next(), t.next()) {
                (Some("ip" | "gs"), Some(addr)) => Some(addr.to_owned()),
                // A bare address is fine too.
                (Some(addr), None) if addr.contains(':') => Some(addr.to_owned()),
                _ => None,
            }
        })
        .collect()
}

fn http_text(url: &str) -> Result<Option<String>> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).http_status_as_error(false).build().into();
    let mut res = agent.get(url).call().with_context(|| format!("GET {url}"))?;
    if res.status().as_u16() == 404 {
        return Ok(None);
    }
    ensure!(res.status().is_success(), "GET {url}: HTTP {}", res.status());
    Ok(Some(res.body_mut().read_to_string()?))
}

/// The curated list: the built-in copy plus the downloaded one.
pub fn curated_list() -> (Vec<String>, Option<anyhow::Error>) {
    let mut out = parse_list(CURATED_BUILTIN);
    let err = match http_text(CURATED_URL) {
        Ok(Some(text)) => {
            out.extend(parse_list(&text));
            None
        }
        // Not pushed yet, or moved: the built-in copy is enough.
        Ok(None) => None,
        Err(e) => Some(e),
    };
    (out, err)
}

/// FWGS's list for `ts`; it has none today (HTTP 404), which isn't an error.
pub fn fwgs_list() -> Result<Vec<String>> {
    Ok(http_text(&format!("{FWGS_LIST_URL}{GAMEDIR}"))?.map(|t| parse_list(&t)).unwrap_or_default())
}

/// Steam's master server through the Web API (needs a key from steamcommunity.com/dev/apikey).
pub fn steam_list(key: &str) -> Result<Vec<String>> {
    let filter = format!("\\appid\\70\\gamedir\\{GAMEDIR}");
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).http_status_as_error(false).build().into();
    let mut res = agent
        .get(STEAM_LIST_URL)
        .query("key", key.trim())
        .query("filter", &filter)
        .query("limit", "500")
        .call()
        .context("Steam Web API")?;
    match res.status().as_u16() {
        200 => {}
        401 | 403 => anyhow::bail!("Steam rejected the API key"),
        s => anyhow::bail!("Steam Web API: HTTP {s}"),
    }
    let json: serde_json::Value = res.body_mut().read_json()?;
    Ok(steam_addresses(&json))
}

/// Steam's master server through the Steam client; false when Steam isn't running.
#[cfg(steam_client)]
fn steam_client_list(found: &mut dyn FnMut(String)) -> Result<bool> {
    crate::steam::server_list(found)
}

#[cfg(not(steam_client))]
fn steam_client_list(_: &mut dyn FnMut(String)) -> Result<bool> {
    Ok(false)
}

fn steam_addresses(json: &serde_json::Value) -> Vec<String> {
    json["response"]["servers"]
        .as_array()
        .map(|a| a.iter().filter_map(|s| s["addr"].as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// Broadcasts A2S_INFO on the LAN and collects TS servers that answer.
pub fn lan_scan(found: &mut dyn FnMut(SocketAddr, ServerInfo, Duration)) -> Result<()> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    s.set_broadcast(true)?;
    let start = Instant::now();
    for port in LAN_PORTS {
        s.send_to(INFO_QUERY, (Ipv4Addr::BROADCAST, port)).context("LAN broadcast")?;
    }
    let mut buf = [0u8; 4096];
    let mut seen = HashSet::new();
    let mut challenged = HashSet::new();
    while let Some(left) = LAN_WAIT.checked_sub(start.elapsed()).filter(|d| !d.is_zero()) {
        s.set_read_timeout(Some(left))?;
        let Ok((n, from)) = s.recv_from(&mut buf) else { break };
        let d = &buf[..n];
        if let Some(c) = challenge(d) {
            if challenged.insert(from) {
                let mut q = INFO_QUERY.to_vec();
                q.extend_from_slice(&c);
                let _ = s.send_to(&q, from);
            }
        } else if let Some(info) = parse_info(d)
            && info.is_ts()
            && seen.insert(from)
        {
            found(from, info, start.elapsed());
        }
    }
    Ok(())
}

/// Progress of a refresh, streamed to the UI.
#[derive(Debug)]
pub enum Event {
    /// `addr` is known as `label` (as written in the source) from `source`.
    Found {
        addr: SocketAddr,
        label: String,
        source: Source,
    },
    /// A source could not be read (resolving one of its names is not reported here).
    SourceFailed {
        source: Source,
        error: String,
    },
    /// A favorite whose name doesn't resolve.
    Unresolved {
        label: String,
        error: String,
    },
    Info {
        addr: SocketAddr,
        result: Result<(ServerInfo, Duration), String>,
    },
    /// Everything has been queried.
    Done,
}

/// Collects addresses from every source and queries each server once, calling `emit` from
/// background threads as results arrive. Returns at once.
pub fn refresh(favorites: Vec<String>, steam_key: String, emit: impl Fn(Event) + Send + Sync + 'static) {
    std::thread::spawn(move || refresh_blocking(favorites, steam_key, Arc::new(emit)));
}

fn refresh_blocking(favorites: Vec<String>, steam_key: String, emit: Arc<dyn Fn(Event) + Send + Sync>) {
    let (tx, rx) = mpsc::channel::<(String, Source)>();
    // Addresses already queried (or answered on the LAN broadcast).
    let queried = Arc::new(Mutex::new(HashSet::<SocketAddr>::new()));
    let mut sources = Vec::new();
    let spawn_source = |source: Source, f: Box<dyn FnOnce() -> Result<Vec<String>> + Send>| {
        let (tx, emit) = (tx.clone(), emit.clone());
        std::thread::spawn(move || match f() {
            Ok(list) => list.into_iter().for_each(|a| drop(tx.send((a, source)))),
            Err(e) => emit(Event::SourceFailed { source, error: format!("{e:#}") }),
        })
    };
    sources.push(spawn_source(Source::Favorite, Box::new(move || Ok(favorites))));
    let emit_list = emit.clone();
    sources.push(spawn_source(
        Source::List,
        Box::new(move || {
            let (mut list, err) = curated_list();
            if let Some(e) = err {
                emit_list(Event::SourceFailed { source: Source::List, error: format!("servers.txt: {e:#}") });
            }
            match fwgs_list() {
                Ok(more) => list.extend(more),
                Err(e) => log::info!("FWGS server list: {e:#}"),
            }
            Ok(list)
        }),
    ));
    {
        let (tx, emit) = (tx.clone(), emit.clone());
        sources.push(std::thread::spawn(move || {
            // The Steam client when it's running, otherwise the Web API with the user's key.
            let client = steam_client_list(&mut |a| drop(tx.send((a, Source::Steam))));
            let failed = |e: anyhow::Error| emit(Event::SourceFailed { source: Source::Steam, error: format!("{e:#}") });
            match client {
                Ok(true) => return,
                Ok(false) => {}
                Err(e) if steam_key.trim().is_empty() => return failed(e),
                Err(e) => log::warn!("{e:#}"),
            }
            if !steam_key.trim().is_empty() {
                match steam_list(&steam_key) {
                    Ok(list) => list.into_iter().for_each(|a| drop(tx.send((a, Source::Steam)))),
                    Err(e) => failed(e),
                }
            }
        }));
    }
    {
        let (emit, queried) = (emit.clone(), queried.clone());
        sources.push(std::thread::spawn(move || {
            let r = lan_scan(&mut |addr, info, ping| {
                queried.lock().unwrap().insert(addr);
                emit(Event::Found { addr, label: addr.to_string(), source: Source::Lan });
                emit(Event::Info { addr, result: Ok((info, ping)) });
            });
            if let Err(e) = r {
                emit(Event::SourceFailed { source: Source::Lan, error: format!("{e:#}") });
            }
        }));
    }
    drop(tx);

    // Resolve and query on a pool of workers; the sources keep feeding it.
    let rx = Arc::new(Mutex::new(rx));
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (rx, emit, queried) = (rx.clone(), emit.clone(), queried.clone());
            std::thread::spawn(move || {
                loop {
                    let Ok((label, source)) = rx.lock().unwrap().recv() else { break };
                    let addr = match resolve(&label) {
                        Ok(a) => a,
                        Err(e) => {
                            if source == Source::Favorite {
                                emit(Event::Unresolved { label, error: format!("{e:#}") });
                            } else {
                                log::info!("server list: {e:#}");
                            }
                            continue;
                        }
                    };
                    emit(Event::Found { addr, label, source });
                    if !queried.lock().unwrap().insert(addr) {
                        continue;
                    }
                    let result = query_info(addr).map_err(|e| format!("{e:#}"));
                    emit(Event::Info { addr, result });
                }
            })
        })
        .collect();
    for t in sources.into_iter().chain(workers) {
        let _ = t.join();
    }
    emit(Event::Done);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goldsrc_info_from_tsmod() {
        let info = parse_info(include_bytes!("../tests/fixtures/a2s/tsmod_info.bin")).unwrap();
        assert_eq!(info.name, "TSMOD.NET - Deathmatch (Moscow)");
        assert_eq!(info.map, "ts_ugah_dm1");
        assert_eq!(info.folder, "ts");
        assert_eq!(info.game, "The Specialists (DM)");
        assert_eq!((info.players, info.max_players, info.bots), (0, 32, 0));
        assert_eq!((info.server_type, info.os, info.password, info.vac), ('d', 'l', false, true));
        assert!(info.is_ts());
    }

    #[test]
    fn source_info_from_fwgs_listen_server() {
        let info = parse_info(include_bytes!("../tests/fixtures/a2s/fwgs_listen_info.bin")).unwrap();
        assert_eq!(info.name, "Launcher test");
        assert_eq!(info.map, "ts_dojo");
        assert_eq!(info.game, "The Specialists (Teamplay)");
        assert_eq!((info.players, info.max_players, info.bots), (4, 8, 3));
        assert_eq!((info.server_type, info.os, info.password), ('l', 'l', false));
    }

    #[test]
    fn players() {
        assert_eq!(parse_players(include_bytes!("../tests/fixtures/a2s/tsmod_player.bin")).unwrap(), []);
        let p = parse_players(include_bytes!("../tests/fixtures/a2s/fwgs_listen_player.bin")).unwrap();
        let names: Vec<_> = p.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Sprayer", "Alpha One", "Bravo", "Charlie"]);
        assert_eq!(p[2].score, 1);
        assert!(p[0].duration > 30.0 && p[1].duration < 0.0);
    }

    #[test]
    fn challenge_reply() {
        let c = include_bytes!("../tests/fixtures/a2s/tsmod_challenge.bin");
        assert_eq!(challenge(c), Some([0xab, 0x4c, 0xf0, 0x71]));
        assert_eq!(challenge(include_bytes!("../tests/fixtures/a2s/tsmod_player.bin")), None);
        assert!(parse_info(c).is_none());
    }

    #[test]
    fn truncated_replies() {
        let full = include_bytes!("../tests/fixtures/a2s/fwgs_listen_player.bin");
        assert!(parse_players(&full[..full.len() - 3]).is_none());
        assert!(parse_info(b"\xff\xff\xff\xffm1.2.3.4:5\0name").is_none());
        assert!(parse_info(b"").is_none());
    }

    #[test]
    fn list_format() {
        let text = "# valve server list\nip 1.2.3.4:27015\n\ngs srv.example.net:27016 # comment\nbogus\n5.6.7.8:27017\n";
        assert_eq!(parse_list(text), ["1.2.3.4:27015", "srv.example.net:27016", "5.6.7.8:27017"]);
        assert!(parse_list(CURATED_BUILTIN).contains(&"srv.tsmod.net:27015".to_owned()));
    }

    #[test]
    fn steam_json() {
        let json = serde_json::json!({"response": {"servers": [{"addr": "1.2.3.4:27015", "gamedir": "ts"}, {"name": "x"}]}});
        assert_eq!(steam_addresses(&json), ["1.2.3.4:27015"]);
        assert!(steam_addresses(&serde_json::json!({"response": {}})).is_empty());
    }

    #[test]
    fn resolves_with_default_port() {
        assert_eq!(resolve("127.0.0.1").unwrap(), "127.0.0.1:27015".parse().unwrap());
        assert_eq!(resolve(" 127.0.0.1:27020 ").unwrap(), "127.0.0.1:27020".parse().unwrap());
    }
}
