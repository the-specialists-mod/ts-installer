//! TS game settings edited outside the game: a catalogue of the cvars players change, and staged
//! edits of `ts/config.cfg`, the Xash3D FWGS `ts/video.cfg` and `ts/userconfig.cfg`.
//!
//! Only archived cvars are listed: the engine rewrites `config.cfg` with archived cvars when it
//! quits, so anything else set there is lost. Defaults are the values the engine or the TS 3.0
//! client registers (from the decompilation, the Rust port and FWGS's source). TS 3.0 has no
//! auto-reload cvar, and forces `default_fov` to 80, so neither is offered.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::cfgfile::{CfgFile, tokenize};
use crate::install::Runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Player,
    Mouse,
    Video,
    Audio,
    Hud,
    Network,
}

impl Section {
    pub const ALL: [Section; 6] = [Self::Player, Self::Mouse, Self::Video, Self::Audio, Self::Hud, Self::Network];

    pub fn title(self) -> &'static str {
        match self {
            Self::Player => "Player",
            Self::Mouse => "Mouse",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Hud => "HUD and gameplay",
            Self::Network => "Network",
        }
    }
}

/// How a setting is edited.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// `0` or `1`.
    Toggle,
    Slider {
        min: f64,
        max: f64,
        step: f64,
    },
    Number {
        min: f64,
        max: f64,
        step: f64,
    },
    /// `(value, label)` pairs.
    Choice(&'static [(&'static str, &'static str)]),
    Text,
    /// The sign of `m_pitch`.
    InvertPitch,
    /// `width` and `height` in `video.cfg`, as `WxH`; `0x0` is the desktop resolution.
    Resolution,
}

/// Which engines have a cvar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engines {
    Both,
    GoldSrc,
    Xash,
}

/// The file a setting lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Config,
    /// Xash3D FWGS `video.cfg`.
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setting {
    pub section: Section,
    /// The cvar (`resolution` stands for `width` + `height`).
    pub cvar: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    pub engines: Engines,
    pub target: Target,
    /// The default on GoldSrc, and on Xash unless `xash_default` is set.
    pub default: &'static str,
    pub xash_default: Option<&'static str>,
}

const fn s(section: Section, cvar: &'static str, label: &'static str, kind: Kind, default: &'static str) -> Setting {
    Setting { section, cvar, label, help: "", kind, engines: Engines::Both, target: Target::Config, default, xash_default: None }
}

impl Setting {
    const fn help(mut self, help: &'static str) -> Self {
        self.help = help;
        self
    }
    const fn xash(mut self, default: &'static str) -> Self {
        self.xash_default = Some(default);
        self
    }
    const fn only(mut self, engines: Engines) -> Self {
        self.engines = engines;
        self
    }
    const fn video(mut self) -> Self {
        self.target = Target::Video;
        self.engines = Engines::Xash;
        self
    }

    pub fn supported(&self, runtime: Runtime) -> bool {
        match self.engines {
            Engines::Both => true,
            Engines::GoldSrc => runtime == Runtime::GoldSrc,
            Engines::Xash => runtime != Runtime::GoldSrc,
        }
    }

    pub fn default_for(&self, runtime: Runtime) -> &'static str {
        match runtime {
            Runtime::Xash(_) => self.xash_default.unwrap_or(self.default),
            Runtime::GoldSrc => self.default,
        }
    }
}

use Kind::*;
use Section::*;

const fn slider(min: f64, max: f64, step: f64) -> Kind {
    Slider { min, max, step }
}
const fn number(min: f64, max: f64, step: f64) -> Kind {
    Number { min, max, step }
}

/// Common resolutions offered for `width` x `height`.
pub const RESOLUTIONS: &[(&str, &str)] = &[
    ("0x0", "Desktop resolution"),
    ("800x600", "800 × 600"),
    ("1024x768", "1024 × 768"),
    ("1280x720", "1280 × 720"),
    ("1280x800", "1280 × 800"),
    ("1280x1024", "1280 × 1024"),
    ("1366x768", "1366 × 768"),
    ("1440x900", "1440 × 900"),
    ("1600x900", "1600 × 900"),
    ("1680x1050", "1680 × 1050"),
    ("1920x1080", "1920 × 1080"),
    ("1920x1200", "1920 × 1200"),
    ("2560x1080", "2560 × 1080"),
    ("2560x1440", "2560 × 1440"),
    ("3440x1440", "3440 × 1440"),
    ("3840x2160", "3840 × 2160"),
];

pub const SETTINGS: &[Setting] = &[
    s(Player, "name", "Name", Text, "Player"),
    // Mouse
    s(Mouse, "sensitivity", "Sensitivity", slider(0.1, 20.0, 0.1), "3"),
    s(Mouse, "m_pitch", "Invert mouse", InvertPitch, "0.022"),
    s(Mouse, "m_rawinput", "Raw input", Toggle, "0").xash("1").help("Read the mouse directly, without the system's acceleration."),
    s(Mouse, "m_filter", "Mouse filter", Toggle, "0").help("Average the movement of two frames."),
    s(Mouse, "look_filter", "Look filter", Toggle, "0").only(Engines::Xash).help("Smooth view movement."),
    s(Mouse, "m_accel", "Acceleration", number(0.0, 10.0, 0.01), "0")
        .help("TS acceleration: adds min(speed ^ exponent × this, limit). 0 is off."),
    s(Mouse, "m_accelexp", "Acceleration exponent", number(0.0, 5.0, 0.1), "1.2"),
    s(Mouse, "m_accelmax", "Acceleration limit", number(0.0, 1000.0, 1.0), "0").help("0 turns acceleration off."),
    // Video
    s(Video, "resolution", "Resolution", Resolution, "0x0").video(),
    s(Video, "fullscreen", "Display mode", Choice(&[("0", "Windowed"), ("1", "Fullscreen"), ("2", "Borderless window")]), "1")
        .video()
        .help("The launcher's \"Windowed\" launch option overrides this."),
    s(Video, "fps_max", "Frame rate limit", number(20.0, 1000.0, 1.0), "100").xash("72"),
    s(Video, "fps_override", "Allow more than 100 fps", Toggle, "0").only(Engines::GoldSrc),
    s(Video, "gl_vsync", "Vertical sync", Toggle, "0").xash("1"),
    s(Video, "brightness", "Brightness", slider(0.0, 2.0, 0.05), "1").xash("0"),
    s(Video, "gamma", "Gamma", slider(1.8, 3.0, 0.05), "2.5"),
    s(Video, "cl_fx", "Special effects detail", Choice(&[("0", "Low"), ("1", "Automatic"), ("2", "High")]), "1"),
    s(Video, "cl_motionblur", "Motion blur", Toggle, "1"),
    s(Video, "cl_shadow", "Player shadows", Choice(&[("0", "None"), ("1", "All players"), ("2", "Other players only")]), "2"),
    s(
        Video,
        "cl_gore",
        "Blood",
        Choice(&[("0", "None"), ("1", "Low"), ("2", "Normal"), ("3", "Lots"), ("4", "Even more"), ("5", "Bloodbath")]),
        "2",
    ),
    s(Video, "cl_wavedetail", "Bullet trails", Toggle, "0"),
    // Audio
    s(Audio, "volume", "Volume", slider(0.0, 1.0, 0.01), "0.8").xash("0.7"),
    s(Audio, "MP3Volume", "Music volume", slider(0.0, 1.0, 0.01), "0.8").xash("1.0"),
    s(Audio, "voice_enable", "Voice chat", Toggle, "1"),
    s(Audio, "voice_scale", "Voice volume", slider(0.0, 2.0, 0.05), "1"),
    // HUD and gameplay
    s(Hud, "crosshair", "Crosshair", Toggle, "1"),
    s(Hud, "cl_solidcross", "Crosshair style", Choice(&[("0", "Transparent"), ("1", "Solid, HUD colour"), ("2", "Solid, black")]), "2"),
    s(Hud, "hud_r", "HUD colour: red", number(0.0, 255.0, 1.0), "150"),
    s(Hud, "hud_g", "HUD colour: green", number(0.0, 255.0, 1.0), "175"),
    s(Hud, "hud_b", "HUD colour: blue", number(0.0, 255.0, 1.0), "255"),
    s(Hud, "cl_realhud", "Realistic HUD", Toggle, "0").help("Hide the status panel."),
    s(
        Hud,
        "cl_showequipment",
        "Show equipment",
        Choice(&[("0", "Never"), ("1", "1/4 transparent"), ("2", "1/2 transparent"), ("3", "Fully transparent")]),
        "1",
    ),
    s(Hud, "hud_fastswitch", "Fast weapon switch", Toggle, "0"),
    s(Hud, "hud_centerid", "Player names in the centre", Toggle, "0"),
    s(Hud, "cl_showfps", "Show frame rate", Toggle, "0"),
    s(Hud, "net_graph", "Network graph", Choice(&[("0", "Off"), ("1", "1"), ("2", "2"), ("3", "3")]), "0"),
    s(Hud, "cl_bob", "View bobbing", Choice(&[("0", "Minimal"), ("1", "1/3"), ("2", "Half"), ("3", "2/3"), ("4", "Full")]), "4"),
    s(Hud, "cl_thirdperson", "Third person view", Toggle, "0"),
    s(
        Hud,
        "cl_camtype",
        "Third person camera",
        Choice(&[("0", "Right shoulder"), ("1", "Left shoulder"), ("2", "Centre"), ("3", "Custom")]),
        "0",
    ),
    s(Hud, "cl_thirdpersonswitch", "Third person for kung fu", Toggle, "0"),
    s(Hud, "cl_thirdtransparency", "Transparent own model when aiming", Toggle, "1"),
    s(Hud, "cl_laser", "Laser sight style", Choice(&[("0", "Standard"), ("1", "Beta 1.0"), ("2", "Realistic")]), "0"),
    s(Hud, "cl_lasercolor", "Laser colour", Choice(&[("10", "Red"), ("1", "Green"), ("2", "Blue")]), "10"),
    s(Hud, "cl_attackswitch", "Swap secondary attack and special", Toggle, "0"),
    s(Hud, "cl_buykevlar", "Buy kevlar by default", Toggle, "1"),
    s(Hud, "cl_usepowerup", "Double-tap use to activate powerups", Toggle, "1"),
    s(Hud, "r_paintball", "Paintball mode", Toggle, "0"),
    // Network
    s(Network, "rate", "Rate (bytes/s)", number(2500.0, 100000.0, 500.0), "30000").xash("25000"),
    s(Network, "cl_updaterate", "Updates per second", number(10.0, 102.0, 1.0), "20"),
    s(Network, "cl_cmdrate", "Commands per second", number(10.0, 100.0, 1.0), "30"),
    s(Network, "ex_interp", "Interpolation (s)", number(0.0, 0.1, 0.005), "0.1").help("0 uses 1 / updates per second."),
    s(Network, "cl_lw", "Client-side weapons", Toggle, "1"),
    s(Network, "cl_lc", "Lag compensation", Toggle, "1"),
];

/// The settings `runtime` has, in `section`.
pub fn settings(runtime: Runtime, section: Section) -> impl Iterator<Item = &'static Setting> {
    SETTINGS.iter().filter(move |s| s.section == section && s.supported(runtime))
}

pub fn setting(cvar: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.cvar.eq_ignore_ascii_case(cvar))
}

/// `v` rounded to the decimals of `step`, without trailing zeros.
pub fn format_number(v: f64, step: f64) -> String {
    let decimals = format!("{step}").split('.').nth(1).map_or(0, str::len);
    let s = format!("{v:.decimals$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

fn same_value(a: &str, b: &str) -> bool {
    a == b || matches!((a.trim().parse::<f64>(), b.trim().parse::<f64>()), (Ok(x), Ok(y)) if x == y)
}

pub fn is_on(value: &str) -> bool {
    value.trim().parse::<f64>().is_ok_and(|v| v != 0.0)
}

/// The files the tab edits, as text.
#[derive(Debug, Clone, PartialEq)]
struct Files {
    config: CfgFile,
    video: Option<CfgFile>,
    userconfig: String,
}

/// Staged edits of one installation's game settings.
#[derive(Debug, Clone)]
pub struct GameConfig {
    pub runtime: Runtime,
    ts_dir: PathBuf,
    config_exists: bool,
    /// Where a new `config.cfg` takes its contents (key bindings) from.
    seed: Option<PathBuf>,
    video_exists: bool,
    video_seed: Option<PathBuf>,
    saved: Files,
    edited: Files,
}

impl GameConfig {
    /// Reads the settings of the game directory `ts_dir` (`<root>/ts`).
    pub fn load(ts_dir: &Path, runtime: Runtime) -> Result<Self> {
        let root = ts_dir.parent().unwrap_or(ts_dir);
        let config_path = ts_dir.join("config.cfg");
        let config_exists = config_path.is_file();
        // Without its own config.cfg the game runs Half-Life's (FWGS falls back to valve/) or the
        // default bindings, so a new file starts from that one to keep the key bindings.
        let mut seeds = vec![ts_dir.join("default.cfg"), root.join("valve/default.cfg")];
        if runtime != Runtime::GoldSrc {
            seeds.insert(0, root.join("valve/config.cfg"));
        }
        let seed = if config_exists { None } else { seeds.into_iter().find(|p| p.is_file()) };
        let config = match &seed {
            Some(p) => CfgFile::parse(&config_path, &read(p)?),
            None => CfgFile::load(&config_path)?,
        };

        let video_path = ts_dir.join("video.cfg");
        let video_exists = video_path.is_file();
        let mut video_seed = None;
        let video = match runtime {
            Runtime::GoldSrc => None,
            Runtime::Xash(_) if video_exists => Some(CfgFile::load(&video_path)?),
            Runtime::Xash(_) => {
                let valve = root.join("valve/video.cfg");
                video_seed = valve.is_file().then_some(valve);
                Some(match &video_seed {
                    Some(p) => CfgFile::parse(&video_path, &read(p)?),
                    None => CfgFile::parse(&video_path, ""),
                })
            }
        };
        let userconfig = match std::fs::read(ts_dir.join("userconfig.cfg")) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).context("read userconfig.cfg"),
        };
        let files = Files { config, video, userconfig };
        Ok(Self {
            runtime,
            ts_dir: ts_dir.to_path_buf(),
            config_exists,
            seed,
            video_exists,
            video_seed,
            saved: files.clone(),
            edited: files,
        })
    }

    pub fn config_path(&self) -> PathBuf {
        self.ts_dir.join("config.cfg")
    }

    pub fn config_exists(&self) -> bool {
        self.config_exists
    }

    /// The file a new `config.cfg` will start from.
    pub fn seed(&self) -> Option<&Path> {
        self.seed.as_deref()
    }

    pub fn video_exists(&self) -> bool {
        self.video_exists
    }

    pub fn video_seed(&self) -> Option<&Path> {
        self.video_seed.as_deref()
    }

    fn file(&self, target: Target) -> Option<&CfgFile> {
        match target {
            Target::Config => Some(&self.edited.config),
            Target::Video => self.edited.video.as_ref(),
        }
    }

    fn file_mut(&mut self, target: Target) -> Option<&mut CfgFile> {
        match target {
            Target::Config => Some(&mut self.edited.config),
            Target::Video => self.edited.video.as_mut(),
        }
    }

    fn raw(&self, target: Target, cvar: &str) -> Option<String> {
        self.file(target)?.get(cvar)
    }

    /// The staged value of `s`, or its default when the file doesn't set it.
    pub fn value(&self, s: &Setting) -> String {
        let default = s.default_for(self.runtime);
        match s.kind {
            Kind::Resolution => {
                let w = self.raw(s.target, "width").unwrap_or_else(|| "0".into());
                let h = self.raw(s.target, "height").unwrap_or_else(|| "0".into());
                format!("{}x{}", w.trim(), h.trim())
            }
            Kind::InvertPitch => {
                let pitch = self.raw(s.target, s.cvar).and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.022);
                if pitch < 0.0 { "1" } else { "0" }.into()
            }
            _ => self.raw(s.target, s.cvar).unwrap_or_else(|| default.to_owned()),
        }
    }

    /// Whether the file sets `s` (otherwise the engine uses the default).
    pub fn is_set(&self, s: &Setting) -> bool {
        match s.kind {
            Kind::Resolution => self.raw(s.target, "width").is_some(),
            _ => self.raw(s.target, s.cvar).is_some(),
        }
    }

    /// Stages `value` for `s`. Unchanged values leave the file alone.
    pub fn set(&mut self, s: &Setting, value: &str) {
        if same_value(&self.value(s), value) {
            return;
        }
        match s.kind {
            Kind::Resolution => {
                let (w, h) = value.split_once('x').unwrap_or(("0", "0"));
                if let Some(f) = self.file_mut(s.target) {
                    f.set("width", w.trim());
                    f.set("height", h.trim());
                }
            }
            Kind::InvertPitch => {
                let pitch = self.raw(s.target, s.cvar).and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.022).abs();
                let pitch = if is_on(value) { -pitch } else { pitch };
                self.set_cvar(s.cvar, &format!("{pitch}"));
            }
            _ => {
                if let Some(f) = self.file_mut(s.target) {
                    f.set(s.cvar, value);
                }
                // The TS client registers hud_color as well; keep it in step with hud_r/g/b.
                if ["hud_r", "hud_g", "hud_b"].contains(&s.cvar) {
                    let c = |n| setting(n).map(|s| self.value(s)).unwrap_or_default();
                    let color = format!("{},{},{}", c("hud_r"), c("hud_g"), c("hud_b"));
                    self.edited.config.set("hud_color", &color);
                }
            }
        }
    }

    /// The `(value, label)` options of a choice or resolution setting, with the current value
    /// added when it isn't one of them.
    pub fn choices(&self, s: &Setting) -> Vec<(String, String)> {
        let base: &[(&str, &str)] = match s.kind {
            Kind::Choice(c) => c,
            Kind::Resolution => RESOLUTIONS,
            _ => &[],
        };
        let mut out: Vec<(String, String)> = base.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect();
        let current = self.value(s);
        if !out.iter().any(|(v, _)| same_value(v, &current)) {
            let label = match s.kind {
                Kind::Resolution => current.replace('x', " × "),
                _ => format!("Other ({current})"),
            };
            out.push((current, label));
        }
        out
    }

    /// Index of the current value in [`Self::choices`].
    pub fn choice_index(&self, s: &Setting) -> usize {
        let current = self.value(s);
        self.choices(s).iter().position(|(v, _)| same_value(v, &current)).unwrap_or(0)
    }

    /// Every cvar `config.cfg` sets (staged).
    pub fn cvars(&self) -> Vec<(String, String)> {
        self.edited.config.cvars()
    }

    /// Stages a raw cvar in `config.cfg`.
    pub fn set_cvar(&mut self, name: &str, value: &str) {
        if self.edited.config.get(name).as_deref() != Some(value) {
            self.edited.config.set(name, value);
        }
    }

    pub fn userconfig(&self) -> &str {
        &self.edited.userconfig
    }

    pub fn set_userconfig(&mut self, text: &str) {
        self.edited.userconfig = text.to_owned();
    }

    /// The cvars whose staged value differs from the files, plus `userconfig.cfg` when edited.
    pub fn changes(&self) -> Vec<String> {
        let diff = |old: &CfgFile, new: &CfgFile| -> Vec<String> {
            new.cvars().into_iter().filter(|(n, v)| old.get(n).as_deref() != Some(v.as_str())).map(|(n, _)| n).collect()
        };
        let mut out = diff(&self.saved.config, &self.edited.config);
        if let (Some(old), Some(new)) = (&self.saved.video, &self.edited.video) {
            out.extend(diff(old, new));
        }
        if self.saved.userconfig != self.edited.userconfig {
            out.push("userconfig.cfg".into());
        }
        out
    }

    pub fn dirty(&self) -> bool {
        self.saved != self.edited
    }

    /// Drops the staged edits.
    pub fn revert(&mut self) {
        self.edited = self.saved.clone();
    }

    /// Writes the edited files (each keeps a `.bak` of its previous version). Returns the files
    /// written.
    pub fn save(&mut self) -> Result<Vec<PathBuf>> {
        let mut written = Vec::new();
        let ucfg = self.ts_dir.join("userconfig.cfg");
        if self.edited.userconfig != self.saved.userconfig || (!ucfg.exists() && !self.edited.userconfig.is_empty()) {
            if ucfg.exists() {
                let _ = std::fs::copy(&ucfg, ucfg.with_extension("cfg.bak"));
            }
            let mut text = self.edited.userconfig.clone();
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            std::fs::write(&ucfg, text).with_context(|| format!("write {}", ucfg.display()))?;
            written.push(ucfg);
        }
        // Both engines write `exec userconfig.cfg` at the end of config.cfg; make sure a file the
        // launcher creates or edits has it too.
        if !self.edited.userconfig.trim().is_empty() && !execs_userconfig(&self.edited.config) {
            let text = self.edited.config.text() + "exec userconfig.cfg\n";
            self.edited.config = CfgFile::parse(&self.edited.config.path, &text);
        }
        if self.edited.config != self.saved.config {
            self.edited.config.save()?;
            written.push(self.edited.config.path.clone());
            self.config_exists = true;
            self.seed = None;
        }
        if let (Some(new), Some(old)) = (&self.edited.video, &self.saved.video)
            && new != old
        {
            new.save()?;
            written.push(new.path.clone());
            self.video_exists = true;
            self.video_seed = None;
        }
        self.saved = self.edited.clone();
        Ok(written)
    }
}

fn execs_userconfig(cfg: &CfgFile) -> bool {
    cfg.text()
        .lines()
        .map(tokenize)
        .any(|t| t.len() == 2 && t[0].eq_ignore_ascii_case("exec") && t[1].eq_ignore_ascii_case("userconfig.cfg"))
}

fn read(path: &Path) -> Result<String> {
    Ok(String::from_utf8_lossy(&std::fs::read(path).with_context(|| format!("read {}", path.display()))?).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::XashBuild;

    const XASH: Runtime = Runtime::Xash(XashBuild::WindowsI386);

    /// A fresh directory with a `ts` game directory under the system temp dir.
    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ts-launcher-gamecfg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("ts")).unwrap();
        std::fs::create_dir_all(root.join("valve")).unwrap();
        root
    }

    fn get(cvar: &str) -> &'static Setting {
        setting(cvar).unwrap()
    }

    #[test]
    fn reads_values_and_defaults() {
        let root = fixture("read");
        std::fs::write(root.join("ts/config.cfg"), "bind \"w\" \"+forward\"\nsensitivity \"5.5\"\nm_pitch \"-0.022\"\n").unwrap();
        let gc = GameConfig::load(&root.join("ts"), XASH).unwrap();
        assert!(gc.config_exists());
        assert_eq!(gc.value(get("sensitivity")), "5.5");
        assert!(gc.is_set(get("sensitivity")));
        assert_eq!(gc.value(get("m_pitch")), "1");
        assert_eq!(gc.value(get("fps_max")), "72");
        assert!(!gc.is_set(get("fps_max")));
        assert_eq!(GameConfig::load(&root.join("ts"), Runtime::GoldSrc).unwrap().value(get("fps_max")), "100");
        // cl_bob 4.0 matches the choice "4"; an unknown laser colour gets its own entry.
        let mut gc = gc;
        gc.set_cvar("cl_bob", "4.0");
        assert_eq!(gc.choices(get("cl_bob"))[gc.choice_index(get("cl_bob"))].1, "Full");
        gc.set_cvar("cl_lasercolor", "0");
        assert_eq!(gc.choices(get("cl_lasercolor")).last().unwrap().1, "Other (0)");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn filters_by_engine() {
        let gold: Vec<_> = settings(Runtime::GoldSrc, Section::Video).map(|s| s.cvar).collect();
        let xash: Vec<_> = settings(XASH, Section::Video).map(|s| s.cvar).collect();
        assert!(gold.contains(&"fps_override") && !gold.contains(&"resolution"));
        assert!(xash.contains(&"resolution") && xash.contains(&"fullscreen") && !xash.contains(&"fps_override"));
        assert!(settings(XASH, Section::Mouse).any(|s| s.cvar == "look_filter"));
        assert!(!settings(Runtime::GoldSrc, Section::Mouse).any(|s| s.cvar == "look_filter"));
    }

    #[test]
    fn writes_changes() {
        let root = fixture("write");
        let original = "// header\nunbindall\nbind \"w\" \"+forward\"\nname \"Old\"\nm_pitch \"0.022\"\nexec userconfig.cfg\n";
        std::fs::write(root.join("ts/config.cfg"), original).unwrap();
        let mut gc = GameConfig::load(&root.join("ts"), Runtime::GoldSrc).unwrap();
        assert!(!gc.dirty());
        gc.set(get("name"), "New");
        gc.set(get("m_pitch"), "1");
        gc.set(get("hud_r"), "10");
        gc.set(get("sensitivity"), "3"); // the default: no line added
        assert!(gc.dirty());
        let mut changes = gc.changes();
        changes.sort();
        assert_eq!(changes, ["hud_color", "hud_r", "m_pitch", "name"]);
        gc.set_userconfig("echo hi");
        let written = gc.save().unwrap();
        assert_eq!(written.len(), 2);
        assert!(!gc.dirty());
        let text = std::fs::read_to_string(root.join("ts/config.cfg")).unwrap();
        assert!(text.contains("name \"New\"") && text.contains("m_pitch \"-0.022\"") && text.contains("hud_color \"10,175,255\""));
        assert!(text.contains("bind \"w\" \"+forward\"") && !text.contains("sensitivity"));
        assert_eq!(text.matches("exec userconfig.cfg").count(), 1);
        assert_eq!(std::fs::read_to_string(root.join("ts/config.cfg.bak")).unwrap(), original);
        assert_eq!(std::fs::read_to_string(root.join("ts/userconfig.cfg")).unwrap(), "echo hi\n");
        // Revert drops staged edits.
        gc.set(get("name"), "Other");
        gc.revert();
        assert_eq!(gc.value(get("name")), "New");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn new_config_starts_from_bindings() {
        let root = fixture("seed");
        std::fs::write(root.join("valve/config.cfg"), "bind \"w\" \"+forward\"\nname \"HL\"\n").unwrap();
        std::fs::write(root.join("valve/default.cfg"), "bind \"a\" \"+moveleft\"\n").unwrap();
        let mut gc = GameConfig::load(&root.join("ts"), XASH).unwrap();
        assert!(!gc.config_exists());
        assert_eq!(gc.seed(), Some(root.join("valve/config.cfg").as_path()));
        assert_eq!(gc.value(get("name")), "HL");
        assert!(gc.save().unwrap().is_empty(), "nothing written without a change");
        gc.set(get("name"), "TS");
        gc.save().unwrap();
        let text = std::fs::read_to_string(root.join("ts/config.cfg")).unwrap();
        assert!(text.contains("bind \"w\" \"+forward\"") && text.contains("name \"TS\""));
        assert!(gc.config_exists());
        // GoldSrc starts from default.cfg.
        std::fs::remove_file(root.join("ts/config.cfg")).unwrap();
        let gc = GameConfig::load(&root.join("ts"), Runtime::GoldSrc).unwrap();
        assert_eq!(gc.seed(), Some(root.join("valve/default.cfg").as_path()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn xash_video_cfg() {
        let root = fixture("video");
        let valve_video = "_window_xpos \"-1\"\nfullscreen \"1\"\nheight \"1080\"\nvid_mode \"0\"\nwidth \"1920\"\n";
        std::fs::write(root.join("valve/video.cfg"), valve_video).unwrap();
        let mut gc = GameConfig::load(&root.join("ts"), XASH).unwrap();
        assert!(!gc.video_exists());
        assert_eq!(gc.video_seed(), Some(root.join("valve/video.cfg").as_path()));
        assert_eq!(gc.value(get("resolution")), "1920x1080");
        assert_eq!(gc.value(get("fullscreen")), "1");
        gc.set(get("resolution"), "1280x720");
        gc.set(get("fullscreen"), "0");
        let mut changes = gc.changes();
        changes.sort();
        assert_eq!(changes, ["fullscreen", "height", "width"]);
        let written = gc.save().unwrap();
        assert_eq!(written, [root.join("ts/video.cfg")]);
        let text = std::fs::read_to_string(root.join("ts/video.cfg")).unwrap();
        assert!(text.contains("width \"1280\"") && text.contains("height \"720\"") && text.contains("fullscreen \"0\""));
        assert!(text.contains("vid_mode \"0\"") && text.contains("_window_xpos \"-1\""));
        // valve/video.cfg is left alone; the game directory's file wins.
        assert_eq!(std::fs::read_to_string(root.join("valve/video.cfg")).unwrap(), valve_video);
        let gc = GameConfig::load(&root.join("ts"), XASH).unwrap();
        assert!(gc.video_exists());
        assert_eq!(gc.value(get("resolution")), "1280x720");
        // An odd resolution shows up as its own choice.
        let mut gc = gc;
        gc.set(get("resolution"), "1000x700");
        assert_eq!(gc.choices(get("resolution"))[gc.choice_index(get("resolution"))].1, "1000 × 700");
        // GoldSrc has no video.cfg.
        assert!(GameConfig::load(&root.join("ts"), Runtime::GoldSrc).unwrap().edited.video.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(format_number(3.0, 0.1), "3");
        assert_eq!(format_number(2.55, 0.05), "2.55");
        assert_eq!(format_number(0.30000001, 0.01), "0.3");
        assert_eq!(format_number(25000.0, 500.0), "25000");
    }
}
