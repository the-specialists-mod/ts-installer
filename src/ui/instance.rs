//! One installation: its details and launch options, plus the server browser, player models,
//! game configuration and controls tabs.

use std::path::Path;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::Disableable;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState, NumberInput, NumberInputEvent, StepAction};
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, IconName, IndexPath, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use ts_launcher::install::{Installation, LibChoice, PortCheck, Runtime, TsLibs, port_manifest, switch_libs, ts_libs, validate_port};
use ts_launcher::progress::{Event, Progress};
use ts_launcher::store::Instance;
use ts_launcher::tsfiles::{self, TsCheck};
use ts_launcher::{hdpack, server_host};

use super::controls::ControlsTab;
use super::gameconfig::GameConfigTab;
use super::models::ModelsTab;
use super::servers::{LaunchAction, LaunchRequest, ServersTab};
use super::{AppState, detail_row, human_bytes, human_time, section, theme};

pub enum InstanceEvent {
    /// Show the installation selector.
    Switch,
    AddInstallation,
    /// This installation was removed from the launcher.
    Removed,
}

#[derive(Clone, Copy, PartialEq)]
enum TabKind {
    Overview,
    Servers,
    Models,
    Config,
    Controls,
}

const TABS: [(TabKind, &str); 5] = [
    (TabKind::Overview, "Overview"),
    (TabKind::Servers, "Server browser"),
    (TabKind::Models, "Player models"),
    (TabKind::Config, "Game configuration"),
    (TabKind::Controls, "Controls"),
];

/// Facts about the files on disk, read when the view opens.
struct Details {
    game: String,
    version: String,
    libs: String,
    valve: bool,
    maps: usize,
    models: usize,
    /// The HD pack release installed in `ts`.
    hd_pack: Option<String>,
}

impl Details {
    fn read(inst: &Instance) -> Self {
        let ts = inst.ts_dir();
        let liblist = std::fs::read_to_string(ts.join("liblist.gam")).unwrap_or_default();
        let field = |key: &str| {
            liblist
                .lines()
                .map(ts_launcher::cfgfile::tokenize)
                .find(|t| t.len() >= 2 && t[0].eq_ignore_ascii_case(key))
                .map(|t| t[1].clone())
                .unwrap_or_default()
        };
        let count = |dir: &Path, f: &dyn Fn(&std::fs::DirEntry) -> bool| {
            std::fs::read_dir(dir).map(|d| d.flatten().filter(|e| f(e)).count()).unwrap_or(0)
        };
        Self {
            game: field("game"),
            version: field("version"),
            libs: ts_libs(&ts).map(|l| l.label().to_owned()).unwrap_or_else(|| "missing".into()),
            valve: ts_launcher::install::is_half_life_dir(&inst.install.root),
            maps: count(&ts.join("maps"), &|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("bsp"))),
            models: count(&ts.join("models/player"), &|e| e.path().is_dir()),
            hd_pack: ts_launcher::hdpack::manifest(&ts).map(|m| m.tag),
        }
    }
}

pub struct InstanceView {
    state: Entity<AppState>,
    id: String,
    tab: TabKind,
    details: Details,
    /// Size of the `ts` folder, computed in the background.
    ts_size: Option<u64>,
    status: Option<Result<String, String>>,
    name: Entity<InputState>,
    extra_args: Entity<InputState>,
    wine: Entity<InputState>,
    wine_prefix: Entity<InputState>,
    servers: Option<Entity<ServersTab>>,
    models: Option<Entity<ModelsTab>>,
    config: Option<Entity<GameConfigTab>>,
    controls: Option<Entity<ControlsTab>>,
    /// The game libraries choices offered for this installation's engine.
    lib_choices: Vec<LibChoice>,
    libs: Entity<SelectState<Vec<SharedString>>>,
    /// Put the libraries select back on what is installed, at the next render.
    resync_libs: bool,
    switching: Option<Switching>,
    /// The Rust port release to install: "Latest release", then each release's tag.
    versions: Entity<SelectState<Vec<SharedString>>>,
    /// The tags behind `versions` after its first item.
    version_tags: Vec<String>,
    /// Releases fetched in the background, for the next render to put in `versions`.
    fetched_versions: Option<Vec<String>>,
    /// After a failed validation: the release to reinstall.
    repair: Option<String>,
    /// Reset was clicked once: ask before doing it.
    confirm_reset: bool,
    /// The local server's map and bot count.
    map: Entity<SelectState<Vec<String>>>,
    bots: Entity<InputState>,
    /// The port's latest release, once the releases are fetched.
    latest_tag: Option<String>,
    /// A launch waiting for the answer to "update the Rust port first?".
    ask_update: Option<Pending>,
    /// A launch to retry once the running job succeeds.
    after_job: Option<Pending>,
    /// The release to lock the port to, once its warning is accepted.
    confirm_lock: Option<String>,
}

/// A button that starts the game, held while the Rust port is offered or updated.
#[derive(Clone, Copy)]
enum Pending {
    Play,
    Servers(LaunchAction),
}

impl Pending {
    /// The button's verb, capitalised and not.
    fn verb(self) -> (&'static str, &'static str) {
        match self {
            Self::Play => ("Play", "play"),
            Self::Servers(LaunchAction::Connect(_)) => ("Join", "join"),
            Self::Servers(LaunchAction::StartServer) => ("Start", "start"),
        }
    }
}

/// Most bots the local server takes (TS servers hold 32 players, one of them you).
const MAX_BOTS: u32 = 31;

/// A game libraries switch or port validation running in the background.
struct Switching {
    title: String,
    bytes: (u64, Option<u64>),
}

enum JobDone {
    Installed { install: Installation, message: String },
    Checked(PortCheck),
    TsChecked(TsCheck),
    TsRepaired(TsCheck),
    HdPack(String),
}

enum SwitchMsg {
    Event(Event),
    Done(Result<JobDone, String>),
}

impl EventEmitter<InstanceEvent> for InstanceView {}

impl InstanceView {
    pub fn new(state: Entity<AppState>, id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inst = state.read(cx).instance(&id).cloned().expect("instance exists");
        let input = |value: &str, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_owned()).default_value(value.to_owned()));
            cx.subscribe(&input, |this, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    this.save_inputs(cx);
                }
            })
            .detach();
            input
        };
        let name = input(&inst.name, "Installation name", window, cx);
        let extra_args = input(&inst.launch.extra_args, "e.g. +connect 1.2.3.4:27015 -dev 2", window, cx);
        let wine = input(&inst.launch.wine, "wine", window, cx);
        let wine_prefix = input(&inst.launch.wine_prefix, "default (~/.wine)", window, cx);
        let bots = input(&inst.launch.bots.to_string(), "0", window, cx);
        cx.subscribe_in(&bots, window, |this, state, e: &NumberInputEvent, window, cx| {
            let NumberInputEvent::Step(action) = e;
            let n: u32 = state.read(cx).value().trim().parse().unwrap_or(0);
            let n = match action {
                StepAction::Increment => (n + 1).min(MAX_BOTS),
                StepAction::Decrement => n.saturating_sub(1),
            };
            state.update(cx, |s, cx| s.set_value(n.to_string(), window, cx));
            this.save_inputs(cx);
        })
        .detach();

        let maps = server_host::maps(&inst.ts_dir());
        let map_ix = match inst.launch.map.as_str() {
            "" => server_host::default_map(&maps).and_then(|m| maps.iter().position(|x| *x == m)),
            m => maps.iter().position(|x| x == m),
        };
        let searchable = maps.len() > 10;
        let map = cx.new(|cx| SelectState::new(maps, map_ix.map(IndexPath::new), window, cx).searchable(searchable));
        cx.subscribe(&map, |this, _, e: &SelectEvent<Vec<String>>, cx| {
            if let SelectEvent::Confirm(Some(m)) = e {
                let m = m.clone();
                this.update_instance(cx, |i| i.launch.map = m);
            }
        })
        .detach();

        let version_tags: Vec<String> = inst.port_tag.iter().cloned().collect();
        let labels = version_labels(&version_tags);
        let versions = cx.new(|cx| SelectState::new(labels, Some(IndexPath::new(version_tags.len())), window, cx));
        cx.subscribe(&versions, |this, state, e: &SelectEvent<Vec<SharedString>>, cx| {
            let SelectEvent::Confirm(Some(_)) = e else { return };
            let ix = state.read(cx).selected_index(cx).map(|i| i.row).unwrap_or(0);
            match ix.checked_sub(1).and_then(|i| this.version_tags.get(i)).cloned() {
                // A release of its own locks the version: warn first.
                Some(tag) => {
                    if this.instance(cx).is_some_and(|i| i.port_tag.as_ref() != Some(&tag)) {
                        this.confirm_lock = Some(tag);
                        cx.notify();
                    }
                }
                None => this.unlock(cx),
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let releases = cx.background_executor().spawn(async { ts_launcher::sources::port_releases() }).await;
            match releases {
                Ok(r) => {
                    let _ = this.update(cx, |this, cx| {
                        this.latest_tag = r.iter().find(|r| !r.prerelease).map(|r| r.tag.clone());
                        this.fetched_versions = Some(r.into_iter().map(|r| r.tag).collect());
                        this.auto_update(cx);
                        cx.notify();
                    });
                }
                Err(e) => log::warn!("listing the Rust port releases: {e:#}"),
            }
        })
        .detach();

        let lib_choices = LibChoice::available(inst.install.runtime);
        let labels: Vec<SharedString> = lib_choices.iter().map(|c| c.label().into()).collect();
        let current = LibChoice::current(&inst.install).and_then(|c| lib_choices.iter().position(|x| *x == c));
        let libs = cx.new(|cx| SelectState::new(labels, current.map(IndexPath::new), window, cx));
        cx.subscribe(&libs, |this, _, e: &SelectEvent<Vec<SharedString>>, cx| {
            let SelectEvent::Confirm(Some(label)) = e else { return };
            if let Some(choice) = this.lib_choices.iter().copied().find(|c| c.label() == label.as_ref()) {
                this.switch_libs(choice, cx);
            }
        })
        .detach();

        let ts = inst.ts_dir();
        cx.spawn(async move |this, cx| {
            let size = cx.background_executor().spawn(async move { dir_size(&ts) }).await;
            let _ = this.update(cx, |this, cx| {
                this.ts_size = Some(size);
                cx.notify();
            });
        })
        .detach();

        Self {
            details: Details::read(&inst),
            state,
            id,
            tab: TabKind::Overview,
            ts_size: None,
            status: None,
            name,
            extra_args,
            wine,
            wine_prefix,
            servers: None,
            models: None,
            config: None,
            controls: None,
            lib_choices,
            libs,
            resync_libs: false,
            switching: None,
            versions,
            version_tags,
            fetched_versions: None,
            repair: None,
            confirm_reset: false,
            map,
            bots,
            latest_tag: None,
            ask_update: None,
            after_job: None,
            confirm_lock: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn instance(&self, cx: &App) -> Option<Instance> {
        self.state.read(cx).instance(&self.id).cloned()
    }

    fn update_instance(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Instance)) {
        let id = self.id.clone();
        let saved = self.state.update(cx, |state, cx| {
            if let Some(inst) = state.store.get_mut(&id) {
                f(inst);
            }
            cx.notify();
            state.save()
        });
        if let Err(e) = saved {
            self.status = Some(Err(e));
        }
        cx.notify();
    }

    fn save_inputs(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        let extra = self.extra_args.read(cx).value().to_string();
        let wine = self.wine.read(cx).value().to_string();
        let prefix = self.wine_prefix.read(cx).value().to_string();
        let bots = self.bots.read(cx).value().trim().parse::<u32>().ok().map(|n| n.min(MAX_BOTS));
        self.update_instance(cx, |inst| {
            if let Some(n) = bots {
                inst.launch.bots = n;
            }
            if !name.is_empty() {
                inst.name = name;
            }
            inst.launch.extra_args = extra;
            inst.launch.wine = wine;
            inst.launch.wine_prefix = prefix;
        });
    }

    fn switch_libs(&mut self, choice: LibChoice, cx: &mut Context<Self>) {
        let Some(inst) = self.instance(cx) else { return };
        if LibChoice::current(&inst.install) == Some(choice) {
            return;
        }
        let tag = inst.port_tag.clone().or_else(|| self.latest_tag.clone());
        self.start_job(format!("Switching to {}", choice.label()), cx, move |progress| {
            let install = switch_libs(&inst.install, choice, tag.as_deref(), progress)?;
            let message = format!("Switched to {} on {}", choice.label(), install.runtime.label());
            Ok(JobDone::Installed { install, message })
        });
    }

    /// Installs Rust port release `tag` (the latest when `None`) over the port already installed;
    /// the same release again only when `force`.
    fn install_version(&mut self, tag: Option<String>, force: bool, cx: &mut Context<Self>) {
        let Some(inst) = self.instance(cx) else { return };
        let Some(choice) = LibChoice::current(&inst.install).filter(|c| *c != LibChoice::Original) else { return };
        let tag = tag.or_else(|| self.latest_tag.clone());
        if !force && tag.is_some() && tag == port_manifest(&inst.ts_dir()).map(|m| m.tag) {
            return;
        }
        let name = tag.clone().unwrap_or_else(|| "the latest release".into());
        self.start_job(format!("Installing Rust port {name}"), cx, move |progress| {
            let install = switch_libs(&inst.install, choice, tag.as_deref(), progress)?;
            let installed = port_manifest(&install.root.join("ts")).map(|m| m.tag).unwrap_or(name);
            Ok(JobDone::Installed { install, message: format!("Installed Rust port {installed} ({})", choice.label()) })
        });
    }

    /// Keeps the port on `tag` (installing it when another release is there) instead of updating it.
    fn lock(&mut self, tag: String, cx: &mut Context<Self>) {
        let pin = tag.clone();
        self.update_instance(cx, |i| i.port_tag = Some(pin));
        self.resync_libs = true;
        self.install_version(Some(tag), false, cx);
    }

    /// Follows the latest port release again, updating to it now.
    fn unlock(&mut self, cx: &mut Context<Self>) {
        self.update_instance(cx, |i| i.port_tag = None);
        self.resync_libs = true;
        self.install_version(None, false, cx);
    }

    /// Whether the installation follows the latest port release but holds an older one.
    fn behind_latest(&self, inst: &Instance) -> bool {
        inst.port_tag.is_none()
            && ts_libs(&inst.ts_dir()) == Some(TsLibs::RustPort)
            && self.latest_tag.as_ref().is_some_and(|latest| port_manifest(&inst.ts_dir()).is_some_and(|m| m.tag != *latest))
    }

    /// Updates the port to the latest release when the installation follows it. A port installed
    /// from a folder or URL has no release to compare, so it stays. Returns whether it started.
    fn auto_update(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(inst) = self.instance(cx) else { return false };
        if self.switching.is_some() || !self.behind_latest(&inst) {
            return false;
        }
        self.install_version(self.latest_tag.clone(), false, cx);
        self.switching.is_some()
    }

    /// Why starting the game should offer the latest port first: the installation runs the
    /// classic TS 3.0 libraries, or stays on an older release.
    fn update_offer(&self, inst: &Instance) -> Option<String> {
        let latest = self.latest_tag.as_ref()?;
        match ts_libs(&inst.ts_dir())? {
            TsLibs::Original => Some(format!("This installation runs the classic TS 3.0 libraries. Rust port {latest} is out.")),
            TsLibs::RustPort => {
                let tag = port_manifest(&inst.ts_dir())?.tag;
                let pos = |t: &str| self.version_tags.iter().position(|x| x == t);
                let older = match (pos(&tag), pos(latest)) {
                    (Some(a), Some(b)) => a > b,
                    _ => tag != *latest,
                };
                let has = if inst.port_tag.is_some() { "is locked to" } else { "has" };
                older.then(|| format!("This installation {has} Rust port {tag}; the latest release is {latest}."))
            }
        }
    }

    /// Starts the game for `pending`, first updating a port that follows the latest release, or
    /// asking whether to update the classic libraries or an older release.
    fn request_launch(&mut self, pending: Pending, cx: &mut Context<Self>) {
        if let Some(s) = &self.switching {
            self.status = Some(Ok(format!("{} once \"{}\" finishes", pending.verb().0, s.title)));
            self.after_job = Some(pending);
            cx.notify();
            return;
        }
        let Some(inst) = self.instance(cx) else { return };
        if self.update_offer(&inst).is_none() {
            return self.run_launch(pending, cx);
        }
        if self.behind_latest(&inst) && self.auto_update(cx) {
            self.after_job = Some(pending);
            return;
        }
        self.ask_update = Some(pending);
        cx.notify();
    }

    fn run_launch(&mut self, pending: Pending, cx: &mut Context<Self>) {
        match pending {
            Pending::Play => self.play(cx),
            Pending::Servers(action) => {
                if let Some(servers) = &self.servers {
                    servers.update(cx, |s, cx| s.run(action, cx));
                }
            }
        }
    }

    /// The answer "update": installs the latest port (over the classic libraries, the build setup
    /// picks), follows the latest release from now on, then starts the game.
    fn update_then_launch(&mut self, pending: Pending, cx: &mut Context<Self>) {
        let Some(inst) = self.instance(cx) else { return };
        let Some(latest) = self.latest_tag.clone() else { return };
        let choice = match LibChoice::current(&inst.install) {
            Some(LibChoice::Original) | None => LibChoice::default_port(inst.install.runtime),
            Some(c) => c,
        };
        self.update_instance(cx, |i| i.port_tag = None);
        self.resync_libs = true;
        self.after_job = Some(pending);
        self.start_job(format!("Updating to Rust port {latest}"), cx, move |progress| {
            let install = switch_libs(&inst.install, choice, Some(&latest), progress)?;
            Ok(JobDone::Installed { install, message: format!("Updated to Rust port {latest} ({})", choice.label()) })
        });
    }

    fn validate(&mut self, cx: &mut Context<Self>) {
        let Some(inst) = self.instance(cx) else { return };
        self.start_job("Validating the Rust port files".into(), cx, move |progress| {
            Ok(JobDone::Checked(validate_port(&inst.install.root, progress)?))
        });
    }

    fn start_job(
        &mut self,
        title: String,
        cx: &mut Context<Self>,
        job: impl FnOnce(&Progress) -> anyhow::Result<JobDone> + Send + 'static,
    ) {
        if self.switching.is_some() {
            return;
        }
        self.status = None;
        self.repair = None;
        self.switching = Some(Switching { title, bytes: (0, None) });
        let (tx, rx) = async_channel::unbounded();
        let events = tx.clone();
        let progress = Progress::new(move |e| {
            let _ = events.send_blocking(SwitchMsg::Event(e));
        });
        std::thread::spawn(move || {
            let result = job(&progress).map_err(|e| format!("{e:#}"));
            let _ = tx.send_blocking(SwitchMsg::Done(result));
        });
        cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(m) = rx.try_recv() {
                    batch.push(m);
                }
                if this.update(cx, |this, cx| batch.into_iter().for_each(|m| this.apply_switch(m, cx))).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn apply_switch(&mut self, msg: SwitchMsg, cx: &mut Context<Self>) {
        match msg {
            SwitchMsg::Event(Event::Step { title, .. }) => {
                if let Some(s) = &mut self.switching {
                    s.title = title;
                    s.bytes = (0, None);
                }
            }
            SwitchMsg::Event(Event::Bytes { done, total }) => {
                if let Some(s) = &mut self.switching {
                    s.bytes = (done, total);
                }
            }
            SwitchMsg::Event(Event::Log(_)) => {}
            SwitchMsg::Done(result) => {
                self.switching = None;
                // Retry a launch that waited for this job; after a failure, the error stands.
                let after = self.after_job.take().filter(|_| result.is_ok());
                match result {
                    Ok(JobDone::Installed { install, message }) => {
                        self.update_instance(cx, |i| i.install = install);
                        if let Some(inst) = self.instance(cx) {
                            self.details = Details::read(&inst);
                        }
                        self.status = Some(Ok(message));
                    }
                    Ok(JobDone::Checked(check)) => {
                        let m = &check.manifest;
                        self.status = Some(if check.ok() {
                            Ok(format!(
                                "All {} Rust port files match {} ({}); the archive matched the SHA-256 published with the release.",
                                check.matching, m.tag, m.asset
                            ))
                        } else {
                            self.repair = Some(m.tag.clone());
                            let bad: Vec<String> = check
                                .different
                                .iter()
                                .map(|f| format!("{f} (differs)"))
                                .chain(check.missing.iter().map(|f| format!("{f} (missing)")))
                                .collect();
                            Err(format!("{} of the files differ from {} ({}): {}", bad.len(), m.tag, m.asset, bad.join(", ")))
                        });
                    }
                    Ok(JobDone::TsChecked(check)) => {
                        self.status = Some(if check.ok() {
                            Ok(format!("All {} TS 3.0 files match the release{}", check.matching, settings_note(&check)))
                        } else {
                            Err(format!(
                                "{} damaged or missing: {}. Repair extracts them again from ts-3-final.exe.{}",
                                files(check.broken().len()),
                                check.broken().join(", "),
                                settings_note(&check)
                            ))
                        });
                    }
                    Ok(JobDone::HdPack(message)) => {
                        if let Some(inst) = self.instance(cx) {
                            self.details = Details::read(&inst);
                        }
                        self.status = Some(Ok(message));
                    }
                    Ok(JobDone::TsRepaired(check)) => {
                        self.status = Some(Ok(match check.broken().len() {
                            0 => format!("Nothing to repair: all {} TS 3.0 files match the release", check.matching),
                            n => format!("Repaired {}: {}", files(n), check.broken().join(", ")),
                        }));
                    }
                    Err(e) => self.status = Some(Err(e)),
                }
                self.resync_libs = true;
                if let Some(pending) = after {
                    self.request_launch(pending, cx);
                }
            }
        }
        cx.notify();
    }

    fn play(&mut self, cx: &mut Context<Self>) {
        self.save_inputs(cx);
        let id = self.id.clone();
        self.status = Some(self.state.update(cx, |state, cx| {
            cx.notify();
            if state.game_running() {
                return Err("The game is already running: quit it first.".to_owned());
            }
            state.play(&id)
        }));
        cx.notify();
    }

    fn render_overview(&mut self, inst: &Instance, cx: &mut Context<Self>) -> AnyElement {
        let d = &self.details;
        let port_installed = ts_libs(&inst.ts_dir()) == Some(ts_launcher::install::TsLibs::RustPort);
        let installed = port_manifest(&inst.ts_dir());
        let busy = self.switching.is_some();
        let needs_wine = matches!(inst.install.runtime, Runtime::Xash(b) if b.needs_wine());
        let launch = &inst.launch;
        let play_args = ts_launcher::launch::play_args(&inst.install, launch);
        let command = ts_launcher::launch::command(&inst.install, launch, &play_args).map(|c| ts_launcher::launch::display(&c));
        let engine_extra = match inst.install.runtime {
            Runtime::GoldSrc if cfg!(windows) => "hl.exe".to_owned(),
            Runtime::GoldSrc => "started through Steam".to_owned(),
            Runtime::Xash(b) => b.executable().to_owned(),
        };
        v_flex()
            .id("overview-scroll")
            .size_full()
            .overflow_y_scrollbar()
            .p_6()
            .child(
                h_flex()
                    .items_start()
                    .gap_4()
                    .child(
                        div().flex_1().min_w_0().child(
                            section("Installation", "", cx)
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .child(
                                            div().w(px(140.)).flex_none().text_sm().text_color(cx.theme().muted_foreground).child("Name"),
                                        )
                                        .child(Input::new(&self.name).small()),
                                )
                                .child(detail_row("Folder", inst.install.root.display().to_string(), cx))
                                .child(detail_row("Engine", format!("{} ({engine_extra})", inst.install.runtime.label()), cx))
                                .child(detail_row(
                                    "Game",
                                    if d.game.is_empty() { "—".to_owned() } else { format!("{} {}", d.game, d.version) },
                                    cx,
                                ))
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .child(
                                            div()
                                                .w(px(140.))
                                                .flex_none()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Game libraries"),
                                        )
                                        .child(
                                            div().w(px(260.)).child(
                                                Select::new(&self.libs)
                                                    .id("game-libs")
                                                    .small()
                                                    .placeholder(d.libs.clone())
                                                    .disabled(self.switching.is_some()),
                                            ),
                                        ),
                                )
                                // The port's version and validation only apply while its libraries are installed.
                                .when(port_installed, |el| {
                                    el.child(
                                        h_flex()
                                            .gap_3()
                                            .child(
                                                div()
                                                    .w(px(140.))
                                                    .flex_none()
                                                    .text_sm()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Rust port version"),
                                            )
                                            .child(div().w(px(260.)).child(
                                                Select::new(&self.versions).id("port-version").small().disabled(self.switching.is_some()),
                                            ))
                                            .child(
                                                Checkbox::new("lock-port")
                                                    .label("Lock this version")
                                                    .checked(inst.port_tag.is_some())
                                                    // Only a release can be locked to.
                                                    .disabled(busy || (inst.port_tag.is_none() && installed.is_none()))
                                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                                        if !*checked {
                                                            return this.unlock(cx);
                                                        }
                                                        let Some(inst) = this.instance(cx) else { return };
                                                        this.confirm_lock = port_manifest(&inst.ts_dir()).map(|m| m.tag);
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                Button::new("validate-port")
                                                    .outline()
                                                    .small()
                                                    .icon(IconName::CircleCheck)
                                                    .label("Validate")
                                                    .disabled(self.switching.is_some())
                                                    .on_click(cx.listener(|this, _, _, cx| this.validate(cx))),
                                            )
                                            .when_some(self.repair.clone(), |r, tag| {
                                                r.child(
                                                    Button::new("repair-port")
                                                        .primary()
                                                        .small()
                                                        .label(format!("Reinstall {tag}"))
                                                        .disabled(self.switching.is_some())
                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                            this.install_version(Some(tag.clone()), true, cx)
                                                        })),
                                                )
                                            }),
                                    )
                                    .child(detail_row(
                                        "",
                                        match &installed {
                                            Some(m) => format!("Installed: {} ({})", m.tag, m.asset),
                                            None => "Installed from a folder or URL, or by an older launcher: no release recorded".into(),
                                        },
                                        cx,
                                    ))
                                })
                                .when_some(self.switching.as_ref(), |el, s| {
                                    let percent = s.bytes.1.filter(|t| *t > 0).map(|t| s.bytes.0 as f32 / t as f32 * 100.);
                                    el.child(
                                        v_flex()
                                            .gap_1()
                                            .pl(px(152.))
                                            .child(div().text_xs().text_color(cx.theme().muted_foreground).child(s.title.clone()))
                                            .child(
                                                ProgressBar::new("switch-progress").loading(percent.is_none()).value(percent.unwrap_or(0.)),
                                            ),
                                    )
                                })
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .child(
                                            div()
                                                .w(px(140.))
                                                .flex_none()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("TS 3.0 files"),
                                        )
                                        .child(
                                            Button::new("validate-ts")
                                                .outline()
                                                .small()
                                                .icon(IconName::CircleCheck)
                                                .label("Validate")
                                                .disabled(busy)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let Some(inst) = this.instance(cx) else { return };
                                                    this.start_job("Validating the TS 3.0 files".into(), cx, move |p| {
                                                        Ok(JobDone::TsChecked(tsfiles::validate(&inst.install.root, p)?))
                                                    });
                                                })),
                                        )
                                        .child(
                                            Button::new("repair-ts")
                                                .outline()
                                                .small()
                                                .icon(IconName::Redo)
                                                .label("Repair")
                                                .disabled(busy)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let Some(inst) = this.instance(cx) else { return };
                                                    this.start_job("Repairing the TS 3.0 files".into(), cx, move |p| {
                                                        Ok(JobDone::TsRepaired(tsfiles::repair(&inst.install.root, p)?))
                                                    });
                                                })),
                                        )
                                        .child(
                                            Button::new("reset-ts")
                                                .ghost()
                                                .small()
                                                .icon(IconName::Undo)
                                                .label("Reset to TS 3.0…")
                                                .disabled(busy)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.confirm_reset = true;
                                                    cx.notify();
                                                })),
                                        ),
                                )
                                .when(self.confirm_reset, |el| {
                                    el.child(
                            v_flex()
                                .pl(px(152.))
                                .gap_2()
                                .child(div().w_full().text_sm().text_color(cx.theme().warning).child(
                                    "Reset puts the original TS 3.0 libraries back (with the engine they need) and extracts every \
                                     release file again, settings included, which also undoes the HD pack. Custom maps and models stay.",
                                ))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(Button::new("reset-cancel").ghost().small().label("Cancel").on_click(cx.listener(
                                            |this, _, _, cx| {
                                                this.confirm_reset = false;
                                                cx.notify();
                                            },
                                        )))
                                        .child(Button::new("reset-confirm").danger().small().label("Reset").on_click(cx.listener(
                                            |this, _, _, cx| {
                                                this.confirm_reset = false;
                                                let Some(inst) = this.instance(cx) else { return };
                                                this.start_job("Resetting to TS 3.0".into(), cx, move |p| {
                                                    let install = tsfiles::reset(&inst.install, p)?;
                                                    let message = format!("Reset to TS 3.0 on {}", install.runtime.label());
                                                    Ok(JobDone::Installed { install, message })
                                                });
                                            },
                                        ))),
                                ),
                        )
                                })
                                .child(
                                    h_flex()
                                        .gap_3()
                                        .child(
                                            div()
                                                .w(px(140.))
                                                .flex_none()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("HD pack"),
                                        )
                                        .child(div().text_sm().child(match &d.hd_pack {
                                            Some(tag) => format!("Installed ({tag})"),
                                            None => "Not installed".to_owned(),
                                        }))
                                        .child(
                                            Button::new("hd-pack-install")
                                                .outline()
                                                .small()
                                                .icon(IconName::ArrowDown)
                                                .label(if d.hd_pack.is_some() { "Reinstall" } else { "Install" })
                                                .disabled(busy)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let Some(inst) = this.instance(cx) else { return };
                                                    this.start_job("Installing the HD pack".into(), cx, move |p| {
                                                        let m = hdpack::install(&inst.install.root, p)?;
                                                        Ok(JobDone::HdPack(format!(
                                                            "Installed the HD pack {} ({} files)",
                                                            m.tag,
                                                            m.files.len()
                                                        )))
                                                    });
                                                })),
                                        )
                                        .when(d.hd_pack.is_some(), |r| {
                                            r.child(
                                                Button::new("hd-pack-remove")
                                                    .ghost()
                                                    .small()
                                                    .icon(IconName::Undo)
                                                    .label("Remove")
                                                    .disabled(busy)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        let Some(inst) = this.instance(cx) else { return };
                                                        this.start_job("Removing the HD pack".into(), cx, move |p| {
                                                            hdpack::remove(&inst.install.root, p)?;
                                                            Ok(JobDone::HdPack(
                                                                "Removed the HD pack: the TS 3.0 models and sounds are back".into(),
                                                            ))
                                                        });
                                                    })),
                                            )
                                        }),
                                )
                                .child(detail_row(
                                    "Half-Life data",
                                    if d.valve { "valve folder present" } else { "valve folder missing" },
                                    cx,
                                ))
                                .child(detail_row("Content", format!("{} maps, {} player models", d.maps, d.models), cx))
                                .child(detail_row("Size of ts", self.ts_size.map(human_bytes).unwrap_or_else(|| "measuring…".into()), cx))
                                .child(detail_row("Added", if inst.created == 0 { "—".into() } else { human_time(inst.created) }, cx))
                                .child(detail_row("Last played", inst.last_played.map(human_time).unwrap_or_else(|| "never".into()), cx)),
                        ),
                    )
                    .child(
                        v_flex()
                            .w(px(360.))
                            .flex_none()
                            .gap_4()
                            .child(
                                Button::new("play")
                                    .primary()
                                    .large()
                                    .w_full()
                                    .icon(IconName::Play)
                                    .label("Play")
                                    .on_click(cx.listener(|this, _, _, cx| this.request_launch(Pending::Play, cx))),
                            )
                            .when_some(self.status.clone(), |d, s| {
                                d.child(match s {
                                    Ok(m) => Alert::success("play-status", m).into_any_element(),
                                    Err(e) => Alert::error("play-status", e).into_any_element(),
                                })
                            })
                            .child(
                                section("Launch options", "", cx)
                                    .child(
                                        v_flex()
                                            .gap_2()
                                            .child(Checkbox::new("windowed").label("Windowed").checked(launch.windowed).on_click(
                                                cx.listener(|this, checked: &bool, _, cx| {
                                                    let checked = *checked;
                                                    this.update_instance(cx, |i| i.launch.windowed = checked);
                                                }),
                                            ))
                                            .child(Checkbox::new("console").label("Developer console").checked(launch.console).on_click(
                                                cx.listener(|this, checked: &bool, _, cx| {
                                                    let checked = *checked;
                                                    this.update_instance(cx, |i| i.launch.console = checked);
                                                }),
                                            ))
                                            .child(
                                                Checkbox::new("local-server")
                                                    .label("Start a local server")
                                                    .checked(launch.local_server)
                                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                                        let checked = *checked;
                                                        this.update_instance(cx, |i| i.launch.local_server = checked);
                                                    })),
                                            ),
                                    )
                                    .when(launch.local_server, |d| {
                                        d.child(
                                            h_flex().gap_3().child(div().w(px(40.)).text_sm().child("Map")).child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .child(Select::new(&self.map).id("local-map").small().placeholder("No maps installed")),
                                            ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_3()
                                                .child(div().w(px(40.)).text_sm().child("Bots"))
                                                .child(div().w(px(120.)).child(NumberInput::new(&self.bots).small())),
                                        )
                                        .child(
                                            div().text_xs().text_color(cx.theme().muted_foreground).child(
                                                "Play hosts this map with you in it and the bots joining a few seconds after it loads. \
                             Game mode, limits and bot skill come from Start a server in the server browser.",
                                            ),
                                        )
                                    })
                                    .child(div().text_sm().child("Extra arguments"))
                                    .child(Input::new(&self.extra_args))
                                    .when(needs_wine, |d| {
                                        d.child(div().text_sm().child("Wine command"))
                                            .child(Input::new(&self.wine))
                                            .child(div().text_sm().child("WINEPREFIX"))
                                            .child(Input::new(&self.wine_prefix))
                                    })
                                    .when_some(command.ok(), |d, c| {
                                        d.child(div().text_xs().font_family("monospace").text_color(cx.theme().muted_foreground).child(c))
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for InstanceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(inst) = self.instance(cx) else {
            return div().into_any_element();
        };
        if let Some(tags) = self.fetched_versions.take() {
            self.version_tags = tags;
            self.resync_libs = true;
        }
        if std::mem::take(&mut self.resync_libs) {
            let current = LibChoice::current(&inst.install).and_then(|c| self.lib_choices.iter().position(|x| *x == c));
            self.libs.update(cx, |s, cx| s.set_selected_index(current.map(IndexPath::new), window, cx));
            // Show the release the installation stays on, or "Latest release" when it follows it.
            if let Some(t) = &inst.port_tag
                && !self.version_tags.contains(t)
            {
                self.version_tags.push(t.clone());
            }
            let ix = inst.port_tag.as_ref().and_then(|t| self.version_tags.iter().position(|x| x == t)).map_or(0, |i| i + 1);
            let labels = version_labels(&self.version_tags);
            self.versions.update(cx, |s, cx| {
                s.set_items(labels, window, cx);
                s.set_selected_index(Some(IndexPath::new(ix)), window, cx);
            });
        }
        let selected = TABS.iter().position(|(k, _)| *k == self.tab).unwrap_or(0);
        let (state, id) = (self.state.clone(), self.id.clone());
        let body = match self.tab {
            TabKind::Overview => self.render_overview(&inst, cx),
            TabKind::Servers => self.servers_tab(window, cx).into_any_element(),
            TabKind::Models => {
                self.models.get_or_insert_with(|| cx.new(|cx| ModelsTab::new(state, id, window, cx))).clone().into_any_element()
            }
            TabKind::Config => {
                self.config.get_or_insert_with(|| cx.new(|cx| GameConfigTab::new(state, id, window, cx))).clone().into_any_element()
            }
            TabKind::Controls => {
                self.controls.get_or_insert_with(|| cx.new(|cx| ControlsTab::new(state, id, window, cx))).clone().into_any_element()
            }
        };
        let several = self.state.read(cx).store.instances.len() > 1;
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_6()
                    .pt_3()
                    .gap_3()
                    .child(super::theme::heading(&inst.name, 22.))
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child(inst.install.runtime.label()))
                    .child(div().flex_1())
                    .when(several, |d| {
                        d.child(
                            Button::new("switch")
                                .ghost()
                                .icon(Lucide::ArrowLeftRight)
                                .tooltip("Switch installation")
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(InstanceEvent::Switch))),
                        )
                    })
                    .child(
                        Button::new("add")
                            .ghost()
                            .icon(IconName::Plus)
                            .tooltip("Add installation")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(InstanceEvent::AddInstallation))),
                    )
                    .child(Button::new("open-folder").ghost().icon(IconName::FolderOpen).tooltip("Open folder").on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(inst) = this.instance(cx) {
                                cx.open_with_system(&inst.install.root);
                            }
                        },
                    )))
                    .child(Button::new("forget").ghost().icon(IconName::Delete).tooltip("Remove from launcher (the files stay)").on_click(
                        cx.listener(|this, _, _, cx| {
                            let id = this.id.clone();
                            let saved = this.state.update(cx, |state, cx| {
                                state.store.remove(&id);
                                cx.notify();
                                state.save()
                            });
                            if let Err(e) = saved {
                                this.status = Some(Err(e));
                            }
                            cx.emit(InstanceEvent::Removed);
                        }),
                    )),
            )
            .child(metal_bar(selected, cx))
            .child(self.state.read(cx).update.clone())
            .child(div().flex_1().min_h_0().relative().child(body).when_some(self.render_update_prompt(&inst, cx), |d, p| d.child(p)))
            .into_any_element()
    }
}

impl InstanceView {
    fn servers_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<ServersTab> {
        if let Some(tab) = &self.servers {
            return tab.clone();
        }
        let (state, id) = (self.state.clone(), self.id.clone());
        let tab = cx.new(|cx| ServersTab::new(state, id, window, cx));
        cx.subscribe(&tab, |this, _, LaunchRequest(action), cx| this.request_launch(Pending::Servers(*action), cx)).detach();
        self.servers = Some(tab.clone());
        tab
    }

    /// Over every tab: the warning before locking the port's version, the question whether to
    /// update the Rust port before starting the game, then that update's progress.
    fn render_update_prompt(&self, inst: &Instance, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = |title: &str, cx: &mut Context<Self>| section(title, "", cx).w(px(520.)).shadow_lg();
        let body = if let Some(tag) = self.confirm_lock.clone() {
            let newer = self.latest_tag.as_ref().filter(|l| **l != tag).map(|l| format!(" {l} is already out.")).unwrap_or_default();
            panel(&format!("Lock Rust port {tag}?"), cx)
                .child(div().text_sm().child(format!("The launcher stops updating this installation and keeps Rust port {tag}.{newer}")))
                .child(div().text_sm().text_color(cx.theme().warning).child(
                    "Servers and players on newer versions may not work with a locked version: joining can fail, \
                     and games can desync or miss fixes and features. Untick \"Lock this version\" to follow the \
                     latest release again.",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .pt_2()
                        .child(Button::new("lock-confirm").danger().icon(Lucide::Lock).label(format!("Lock {tag}")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.confirm_lock = None;
                                this.lock(tag.clone(), cx);
                            },
                        )))
                        .child(Button::new("lock-cancel").ghost().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_lock = None;
                            // Put the version select back.
                            this.resync_libs = true;
                            cx.notify();
                        }))),
                )
        } else if let Some(pending) = self.ask_update {
            let (verb, verb_lower) = pending.verb();
            let latest = self.latest_tag.clone().unwrap_or_default();
            let reason = self.update_offer(inst).unwrap_or_default();
            panel("Update the Rust port?", cx)
                .child(div().text_sm().child(reason))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child(format!(
                    "Updating installs {latest} and keeps this installation on the latest release from now on (unlocking its version)."
                )))
                .child(
                    h_flex()
                        .gap_2()
                        .pt_2()
                        .child(
                            Button::new("update-launch")
                                .primary()
                                .icon(IconName::ArrowDown)
                                .label(format!("Update and {verb_lower}"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.ask_update = None;
                                    this.update_then_launch(pending, cx);
                                })),
                        )
                        .child(Button::new("launch-anyway").outline().label(format!("{verb} without updating")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.ask_update = None;
                                this.run_launch(pending, cx);
                            },
                        )))
                        .child(Button::new("update-cancel").ghost().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                            this.ask_update = None;
                            cx.notify();
                        }))),
                )
        } else if let (Some(pending), Some(s)) = (self.after_job, &self.switching) {
            let percent = s.bytes.1.filter(|t| *t > 0).map(|t| s.bytes.0 as f32 / t as f32 * 100.);
            panel("Update the Rust port?", cx)
                .child(div().text_sm().child(format!("{}; the game starts when it's done.", s.title)))
                .child(ProgressBar::new("launch-update-progress").loading(percent.is_none()).value(percent.unwrap_or(0.)))
                .child(h_flex().pt_2().child(
                    Button::new("launch-update-cancel").ghost().small().label(format!("Don't {} after", pending.verb().1)).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.after_job = None;
                            this.status = None;
                            cx.notify();
                        }),
                    ),
                ))
        } else {
            return None;
        };
        Some(
            div()
                .id("update-prompt")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.6))
                // Keep clicks off the tab underneath.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(body)
                .into_any_element(),
        )
    }
}

/// The tabs as the site's nav bar: stencilled labels on brushed metal, the selected one lit.
fn metal_bar(selected: usize, cx: &mut Context<InstanceView>) -> impl IntoElement {
    div()
        .relative()
        .mt_3()
        .h(px(41.))
        .border_y_1()
        .border_color(cx.theme().border)
        .child(img(theme::METAL_BAR.clone()).absolute().size_full().object_fit(ObjectFit::Fill))
        .child(h_flex().relative().size_full().px_4().justify_center().children(TABS.iter().enumerate().map(|(ix, (kind, label))| {
            let lit = ix == selected;
            let kind = *kind;
            div()
                .id(ix)
                .h_full()
                .px_3()
                .flex()
                .items_center()
                .cursor_pointer()
                .font_family(theme::DISPLAY_FONT)
                .text_size(px(19.))
                .text_color(if lit { theme::stencil_lit() } else { theme::stencil() })
                .when(lit, |d| d.bg(hsla(0., 0., 0., 0.55)))
                .hover(|s| s.text_color(theme::stencil_lit()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = kind;
                    cx.notify();
                }))
                .child(label.to_uppercase())
        })))
}

/// "1 file" or "n files".
fn files(n: usize) -> String {
    if n == 1 { "1 file".into() } else { format!("{n} files") }
}

/// Which settings differ from the release's defaults, as a sentence to append.
fn settings_note(check: &TsCheck) -> String {
    match check.settings_changed.as_slice() {
        [] => String::new(),
        s => format!(" ({} changed from the defaults, as expected)", s.join(", ")),
    }
}

/// The version select's items: "Latest release", then the tags.
fn version_labels(tags: &[String]) -> Vec<SharedString> {
    std::iter::once("Latest release".into()).chain(tags.iter().map(|t| t.clone().into())).collect()
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}
