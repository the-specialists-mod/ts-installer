//! Starting your own TS server: a listen server (the game client hosts the map) or, where the
//! engine ships one, a dedicated server, with bots.
//!
//! Game cvars can't go on the command line: the engine runs `+cvar` before the game library
//! registers them, and registering resets them. So the launcher writes `ts/tslauncher_server.cfg`
//! and points `lservercfgfile`/`servercfgfile` at it; TS's game rules exec that file once each map
//! has loaded. The first time it runs it reloads the map with `changelevel` (the mode cvars, like
//! `mp_teamplay`, are read when a map starts); after that it waits two seconds and adds the bots
//! from `ts/tslauncher_bots.cfg`. Bots are dropped on every map change, so they are added again on
//! each map. Waits use aliases: FWGS's command buffer is 32 KB and long `wait` chains overflow it.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::install::{Installation, Runtime, TsLibs};
use crate::sources::XashBuild;

pub const SERVER_CFG: &str = "tslauncher_server.cfg";
pub const BOTS_CFG: &str = "tslauncher_bots.cfg";
/// The dedicated server's console output, in `ts/`.
pub const DEDICATED_LOG: &str = "tslauncher_server.log";
/// The player models TS's bots pick from; with teams on, a model decides the team.
pub const BOT_MODELS: [&str; 7] = ["seal", "merc", "gordon", "hitman", "castor", "agent", "laurence"];
/// Seconds between the map starting and the first bot joining.
const BOT_DELAY_SECS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameMode {
    Deathmatch,
    TeamDeathmatch,
    LastManStanding,
    TheOne,
}

impl GameMode {
    pub const ALL: [Self; 4] = [Self::Deathmatch, Self::TeamDeathmatch, Self::LastManStanding, Self::TheOne];

    pub fn label(self) -> &'static str {
        match self {
            Self::Deathmatch => "Deathmatch",
            Self::TeamDeathmatch => "Team deathmatch",
            Self::LastManStanding => "Last man standing",
            Self::TheOne => "The One",
        }
    }
}

/// How the server runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerKind {
    /// The game itself hosts the map and you play on it.
    Listen,
    /// A separate server process without graphics.
    Dedicated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BotSettings {
    pub count: u32,
    /// 0.1 (weak) to 1.0; `None` picks a random skill for each bot.
    pub skill: Option<f32>,
    /// A model from [`BOT_MODELS`] (which also picks the team with teams on); empty is random.
    pub team: String,
    /// Bots learn the map's routes (`botlearn`).
    pub learn: bool,
    /// Bots chat (`bottalk`).
    pub talk: bool,
    /// Bots spawn with armor (`botarmor`).
    pub armor: bool,
}

impl Default for BotSettings {
    fn default() -> Self {
        Self { count: 0, skill: Some(0.5), team: String::new(), learn: true, talk: true, armor: false }
    }
}

/// Everything the "Start a server" form sets, saved between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostSettings {
    pub kind: ServerKind,
    /// With a dedicated server: also start the game and join it.
    pub join: bool,
    pub hostname: String,
    pub map: String,
    pub max_players: u32,
    pub port: u16,
    pub password: String,
    /// Only LAN clients may join (`sv_lan`).
    pub lan_only: bool,
    pub mode: GameMode,
    /// Minutes; 0 = no limit.
    pub time_limit: u32,
    /// 0 = no limit.
    pub frag_limit: u32,
    /// Seconds per round (team and last-man-standing rounds).
    pub round_time: u32,
    pub respawn_time: u32,
    pub friendly_fire: bool,
    pub powerups: bool,
    /// Simulated bullets (`realbullet`).
    pub real_bullets: bool,
    /// The buy system with money (`usecash`).
    pub use_cash: bool,
    /// Slow motion (`slowmatch`).
    pub slow_motion: bool,
    pub bots: BotSettings,
}

impl Default for HostSettings {
    fn default() -> Self {
        Self {
            kind: ServerKind::Listen,
            join: true,
            hostname: "My TS server".into(),
            map: "ts_dojo".into(),
            max_players: 16,
            port: 27015,
            password: String::new(),
            lan_only: false,
            mode: GameMode::Deathmatch,
            time_limit: 20,
            frag_limit: 0,
            round_time: 150,
            respawn_time: 3,
            friendly_fire: false,
            powerups: true,
            real_bullets: false,
            use_cash: false,
            slow_motion: true,
            bots: BotSettings::default(),
        }
    }
}

impl HostSettings {
    fn path() -> PathBuf {
        crate::store::home().join("server.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?).with_context(|| format!("write {}", path.display()))
    }

    /// Problems that stop the server from starting.
    pub fn validate(&self, ts: &Path) -> Result<()> {
        if self.map.is_empty() {
            bail!("choose a map");
        }
        if !ts.join("maps").join(format!("{}.bsp", self.map)).is_file() {
            bail!("ts/maps/{}.bsp doesn't exist", self.map);
        }
        if !(2..=32).contains(&self.max_players) {
            bail!("max players must be 2 to 32");
        }
        if self.port == 0 {
            bail!("choose a port");
        }
        if self.bots.count >= self.max_players {
            bail!("{} bots leave no room for players: allow more players or add fewer bots", self.bots.count);
        }
        Ok(())
    }
}

/// How bots can be added to a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotSupport {
    /// `addcustombot name model skill` (TS 3.0 and the Rust port on a listen server).
    Commands,
    /// The Rust port's `ts_bot_quota` on a dedicated server; names come from the port.
    Quota,
    /// TS 3.0's bots only run on listen servers.
    None,
}

pub fn bot_support(kind: ServerKind, libs: Option<TsLibs>) -> BotSupport {
    match (kind, libs) {
        (ServerKind::Listen, _) => BotSupport::Commands,
        (ServerKind::Dedicated, Some(TsLibs::RustPort)) => BotSupport::Quota,
        (ServerKind::Dedicated, _) => BotSupport::None,
    }
}

/// The dedicated server binary of this install, if its engine has one: the Linux FWGS build ships
/// `xash` next to `xash3d`. Windows FWGS has no dedicated build, and HLDS isn't set up here.
pub fn dedicated_binary(install: &Installation) -> Option<PathBuf> {
    match install.runtime {
        Runtime::Xash(XashBuild::LinuxAmd64 | XashBuild::LinuxI386) => Some(install.root.join("xash")).filter(|p| p.is_file()),
        _ => None,
    }
}

/// The maps in `ts/maps`, sorted.
pub fn maps(ts: &Path) -> Vec<String> {
    let mut maps: Vec<String> = std::fs::read_dir(ts.join("maps"))
        .map(|d| {
            d.flatten()
                .filter_map(|e| {
                    let p = e.path();
                    p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bsp")).then(|| p.file_stem()?.to_str().map(str::to_owned))?
                })
                .collect()
        })
        .unwrap_or_default();
    maps.sort_by_key(|m| m.to_lowercase());
    maps
}

/// The map to offer when none is chosen: `ts_dojo` if installed, else the first map.
pub fn default_map(maps: &[String]) -> Option<String> {
    maps.iter().find(|m| *m == "ts_dojo").or(maps.first()).cloned()
}

/// Bot names from `ts/bots.txt` (one per line; TS reads at most 64, `name//skill` sets a skill).
pub fn read_bot_names(ts: &Path) -> Vec<String> {
    std::fs::read_to_string(ts.join("bots.txt")).map(|s| parse_bot_names(&s)).unwrap_or_default()
}

pub fn parse_bot_names(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split("//").next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.chars().take(31).collect::<String>())
        .collect()
}

/// Writes `ts/bots.txt` from the names in `text` (kept as typed, `name//skill` included), keeping
/// the previous file as `bots.txt.bak`. Does nothing when the names are unchanged.
pub fn write_bot_names(ts: &Path, text: &str) -> Result<bool> {
    let path = ts.join("bots.txt");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    let old_lines: Vec<&str> = old.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    if lines == old_lines {
        return Ok(false);
    }
    if path.is_file() {
        std::fs::copy(&path, ts.join("bots.txt.bak")).context("back up bots.txt")?;
    }
    // TS 3.0 ships it with Windows line endings.
    std::fs::write(&path, lines.iter().map(|l| format!("{l}\r\n")).collect::<String>())
        .with_context(|| format!("write {}", path.display()))?;
    Ok(true)
}

/// A value safe inside a quoted cfg argument: no quotes, `;` or line breaks.
fn cfg_quote(s: &str) -> String {
    let s: String = s.chars().filter(|c| !matches!(c, '"' | ';' | '\n' | '\r')).collect();
    format!("\"{}\"", s.trim())
}

fn on(b: bool) -> u8 {
    b.into()
}

/// The cfg the server execs on every map.
pub fn server_cfg(s: &HostSettings, bots: BotSupport) -> String {
    let mode = s.mode;
    let mut c = String::from("// Written by the TS launcher (\"Start a server\"); it is rewritten every time a server starts.\n");
    c += &format!("alias tsl_w10 \"{}\"\n", ["wait"; 10].join(";"));
    c += &format!("alias tsl_w100 \"{}\"\n", ["tsl_w10"; 10].join(";"));
    c += &format!("hostname {}\n", cfg_quote(&s.hostname));
    c += &format!("sv_password {}\n", cfg_quote(&s.password));
    c += &format!("sv_lan {}\n", on(s.lan_only));
    c += &format!("mp_teamplay {}\n", on(mode == GameMode::TeamDeathmatch));
    c += &format!("lastmanstanding {}\n", on(mode == GameMode::LastManStanding));
    c += &format!("mp_theonemode {}\n", on(mode == GameMode::TheOne));
    c += &format!("mp_timelimit {}\n", s.time_limit);
    c += &format!("mp_fraglimit {}\n", s.frag_limit);
    c += &format!("roundtime {}\n", s.round_time);
    c += &format!("respawntime {}\n", s.respawn_time);
    c += &format!("mp_friendlyfire {}\n", on(s.friendly_fire));
    c += &format!("nopowerup {}\n", on(!s.powerups));
    c += &format!("realbullet {}\n", on(s.real_bullets));
    c += &format!("usecash {}\n", on(s.use_cash));
    c += &format!("slowmatch {}\n", on(s.slow_motion));
    c += &format!("botlearn {}\n", on(s.bots.learn));
    c += &format!("bottalk {}\n", on(s.bots.talk));
    c += &format!("botarmor {}\n", on(s.bots.armor));
    let add_bots = match bots {
        BotSupport::Commands if s.bots.count > 0 => {
            format!("{}exec {BOTS_CFG}", "tsl_w100;".repeat(BOT_DELAY_SECS))
        }
        _ => "echo".into(),
    };
    if bots == BotSupport::Quota {
        c += &format!("ts_bot_quota {}\n", s.bots.count);
    }
    // `tsl_stage` starts as `tsl_restart` (set on the command line), so the first map reloads once
    // with these settings in force; from then on every map adds the bots.
    c += &format!("alias tsl_bots \"{add_bots}\"\n");
    c += &format!("alias tsl_restart \"alias tsl_stage tsl_bots; changelevel {}\"\n", s.map);
    c += "tsl_stage\n";
    c
}

/// The cfg that adds the bots: `addcustombot "name" model skill` for each, one frame apart.
/// `random` returns a number in `0..n` (names and random models and skills).
pub fn bots_cfg(s: &HostSettings, names: &[String], mut random: impl FnMut(usize) -> usize) -> String {
    let mut pool: Vec<String> = names.to_vec();
    let mut c = String::from("// Written by the TS launcher: the bots for \"Start a server\".\n");
    for i in 0..s.bots.count {
        let name = if pool.is_empty() { format!("Bot {}", i + 1) } else { pool.remove(random(pool.len())) };
        let model = if s.bots.team.is_empty() { BOT_MODELS[random(BOT_MODELS.len())] } else { s.bots.team.as_str() };
        let skill = s.bots.skill.unwrap_or_else(|| 0.1 + random(10) as f32 * 0.1).clamp(0.1, 1.0);
        c += &format!("addcustombot {} {} {skill:.2}\nwait\n", cfg_quote(&name), cfg_quote(model));
    }
    c
}

/// Writes both cfgs into `ts/`.
pub fn write_cfgs(ts: &Path, s: &HostSettings, bots: BotSupport) -> Result<()> {
    let names = read_bot_names(ts);
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(1);
    let mut state = u64::from(seed) | 1;
    let random = move |n: usize| {
        // xorshift: plenty for shuffling bot names.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n.max(1) as u64) as usize
    };
    std::fs::write(ts.join(SERVER_CFG), server_cfg(s, bots)).with_context(|| format!("write ts/{SERVER_CFG}"))?;
    std::fs::write(ts.join(BOTS_CFG), bots_cfg(s, &names, random)).with_context(|| format!("write ts/{BOTS_CFG}"))?;
    Ok(())
}

/// Arguments for the game to host a listen server (after the launch options).
pub fn listen_args(s: &HostSettings) -> Vec<String> {
    let mut a: Vec<String> = vec!["-port".into(), s.port.to_string()];
    a.extend(["+maxplayers".into(), s.max_players.to_string()]);
    a.extend(["+lservercfgfile".into(), SERVER_CFG.into()]);
    a.extend(["+alias".into(), "tsl_stage".into(), "tsl_restart".into()]);
    a.extend(["+map".into(), s.map.clone()]);
    a
}

/// The command that starts a dedicated server for `install`.
pub fn dedicated_command(install: &Installation, s: &HostSettings) -> Result<Command> {
    let exe = dedicated_binary(install).context("this engine has no dedicated server")?;
    let mut cmd = Command::new(exe);
    cmd.args(["-game", "ts", "-port", &s.port.to_string()]);
    cmd.args(["+maxplayers", &s.max_players.to_string(), "+servercfgfile", SERVER_CFG, "+alias", "tsl_stage", "tsl_restart"]);
    cmd.args(["+map", &s.map]);
    cmd.current_dir(&install.root).env("XASH3D_BASEDIR", &install.root);
    Ok(cmd)
}

/// Starts the dedicated server with its console output in `ts/tslauncher_server.log`.
pub fn start_dedicated(install: &Installation, s: &HostSettings) -> Result<Child> {
    let mut cmd = dedicated_command(install, s)?;
    let log = std::fs::File::create(install.root.join("ts").join(DEDICATED_LOG)).context("create the server log")?;
    cmd.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    log::info!("starting dedicated server {cmd:?}");
    cmd.spawn().with_context(|| format!("start {:?}", cmd.get_program()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_cfg_sets_mode_and_stages() {
        let mut s = HostSettings { mode: GameMode::TeamDeathmatch, password: "se\"cr;et".into(), ..Default::default() };
        s.bots.count = 3;
        let c = server_cfg(&s, BotSupport::Commands);
        assert!(c.contains("mp_teamplay 1\n") && c.contains("lastmanstanding 0\n") && c.contains("mp_theonemode 0\n"));
        assert!(c.contains("sv_password \"secret\"\n"));
        assert!(c.contains("alias tsl_restart \"alias tsl_stage tsl_bots; changelevel ts_dojo\"\n"));
        assert!(c.contains("alias tsl_bots \"tsl_w100;tsl_w100;exec tslauncher_bots.cfg\"\n"));
        assert!(c.ends_with("tsl_stage\n"));
        assert!(!c.contains("ts_bot_quota"));
        // No raw wait chain longer than one alias.
        assert!(c.lines().all(|l| l.matches("wait").count() <= 10));

        let q = server_cfg(&s, BotSupport::Quota);
        assert!(q.contains("ts_bot_quota 3\n") && q.contains("alias tsl_bots \"echo\"\n"));
        s.bots.count = 0;
        assert!(server_cfg(&s, BotSupport::Commands).contains("alias tsl_bots \"echo\"\n"));
    }

    #[test]
    fn bots_cfg_uses_names_once() {
        let bots = BotSettings { count: 3, skill: Some(0.7), team: "merc".into(), ..Default::default() };
        let mut s = HostSettings { bots, ..Default::default() };
        let names = vec!["Le Marc".to_owned(), "Rambo".to_owned()];
        let c = bots_cfg(&s, &names, |_| 0);
        let lines: Vec<_> = c.lines().filter(|l| l.starts_with("addcustombot")).collect();
        assert_eq!(
            lines,
            ["addcustombot \"Le Marc\" \"merc\" 0.70", "addcustombot \"Rambo\" \"merc\" 0.70", "addcustombot \"Bot 3\" \"merc\" 0.70"]
        );
        s.bots.team.clear();
        s.bots.skill = None;
        let c = bots_cfg(&s, &names, |n| n - 1);
        assert!(c.contains("addcustombot \"Rambo\" \"laurence\" 1.00"));
    }

    #[test]
    fn bot_names_file() {
        assert_eq!(parse_bot_names("Lombardi\r\n\r\nWinters//0.5\n  \nLe Marc  \n"), ["Lombardi", "Winters", "Le Marc"]);
        let dir = std::env::temp_dir().join(format!("tsl-bots-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bots.txt"), "A\r\nB\r\n").unwrap();
        assert!(!write_bot_names(&dir, "A\nB\n\n").unwrap());
        assert!(write_bot_names(&dir, "A\nC//0.9\n").unwrap());
        assert_eq!(std::fs::read_to_string(dir.join("bots.txt")).unwrap(), "A\r\nC//0.9\r\n");
        assert_eq!(std::fs::read_to_string(dir.join("bots.txt.bak")).unwrap(), "A\r\nB\r\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn listen_args_end_with_map() {
        let s = HostSettings { port: 27031, max_players: 8, ..Default::default() };
        let a = listen_args(&s).join(" ");
        assert_eq!(a, "-port 27031 +maxplayers 8 +lservercfgfile tslauncher_server.cfg +alias tsl_stage tsl_restart +map ts_dojo");
    }

    #[test]
    fn bots_by_server_kind() {
        assert_eq!(bot_support(ServerKind::Listen, Some(TsLibs::Original)), BotSupport::Commands);
        assert_eq!(bot_support(ServerKind::Dedicated, Some(TsLibs::RustPort)), BotSupport::Quota);
        assert_eq!(bot_support(ServerKind::Dedicated, Some(TsLibs::Original)), BotSupport::None);
    }
}
