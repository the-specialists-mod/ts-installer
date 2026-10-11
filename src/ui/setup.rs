//! Adding an installation: the welcome choice, the setup form, and install progress.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::base::Disableable;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use ts_launcher::detect;
use ts_launcher::install::{Engine, EngineSource, Existing, GameLibs, Installation, Plan, PortSource, TsLibs, TsSource, ValveSource};
use ts_launcher::progress::{Event, Progress};

use super::{Launcher, Screen, human_bytes, path_text, section};

const LOG_LINES: usize = 300;

/// Which path a file dialog fills in.
#[derive(Clone, Copy)]
enum PathField {
    HalfLife,
    XashRoot,
    ValveFrom,
    TsLocal,
    PortDir,
}

#[derive(Clone, Copy, PartialEq)]
enum ValveChoice {
    Keep,
    Download,
    Copy,
}

#[derive(Clone, Copy, PartialEq)]
enum TsChoice {
    Keep,
    Download,
    Local,
}

enum Msg {
    Event(Event),
    Done(Result<Installation, String>),
}

/// The setup form's choices and the install in progress.
pub struct Setup {
    use_xash: bool,
    half_life: Option<PathBuf>,
    detected: Vec<PathBuf>,
    xash_root: PathBuf,
    /// What the chosen folder already contains.
    existing: Existing,
    keep_engine: bool,
    valve: ValveChoice,
    valve_from: Option<PathBuf>,
    ts: TsChoice,
    ts_path: Option<PathBuf>,
    rust_port: bool,
    port_from_url: bool,
    port_dir: Option<PathBuf>,
    port_url: Entity<InputState>,

    steps: Vec<String>,
    step: usize,
    bytes: (u64, Option<u64>),
    log: Vec<String>,
    error: Option<String>,
    cancel: Option<Arc<AtomicBool>>,
}

impl Setup {
    pub fn new(detected: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Launcher>) -> Self {
        let port_url = cx.new(|cx| InputState::new(window, cx).placeholder("https://…/ts-rust-dist.zip"));
        cx.subscribe(&port_url, |_, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        let mut setup = Self {
            use_xash: false,
            half_life: detected.first().cloned(),
            detected,
            xash_root: detect::default_xash_dir(),
            existing: Existing::default(),
            keep_engine: false,
            valve: ValveChoice::Download,
            valve_from: None,
            ts: TsChoice::Download,
            ts_path: None,
            rust_port: false,
            port_from_url: false,
            port_dir: None,
            port_url,
            steps: Vec::new(),
            step: 0,
            bytes: (0, None),
            log: Vec::new(),
            error: None,
            cancel: None,
        };
        setup.refresh_existing();
        setup
    }

    fn root(&self) -> PathBuf {
        if self.use_xash { self.xash_root.clone() } else { self.half_life.clone().unwrap_or_default() }
    }

    /// Re-inspects the chosen folder and defaults every choice to keeping what is there.
    fn refresh_existing(&mut self) {
        self.existing = Existing::inspect(&self.root());
        self.keep_engine = self.existing.xash.is_some();
        self.valve = match (self.existing.valve, self.valve) {
            (true, _) => ValveChoice::Keep,
            (false, ValveChoice::Keep) => ValveChoice::Download,
            (false, v) => v,
        };
        self.ts = match (self.existing.ts.is_some(), self.ts) {
            (true, _) => TsChoice::Keep,
            (false, TsChoice::Keep) => TsChoice::Download,
            (false, t) => t,
        };
    }

    fn valve_choices(&self) -> Vec<ValveChoice> {
        let mut v = vec![ValveChoice::Download, ValveChoice::Copy];
        if self.existing.valve {
            v.insert(0, ValveChoice::Keep);
        }
        v
    }

    fn ts_choices(&self) -> Vec<TsChoice> {
        let mut v = vec![TsChoice::Download, TsChoice::Local];
        if self.existing.ts.is_some() {
            v.insert(0, TsChoice::Keep);
        }
        v
    }

    fn plan(&self, cx: &App) -> Plan {
        let engine = if self.use_xash {
            Engine::Xash {
                engine: match (self.keep_engine, self.existing.xash) {
                    (true, Some(build)) => EngineSource::Keep(build),
                    _ => EngineSource::Download,
                },
                valve: match self.valve {
                    ValveChoice::Keep => ValveSource::Keep,
                    ValveChoice::Download => ValveSource::XashTs,
                    ValveChoice::Copy => ValveSource::CopyFrom(self.valve_from.clone().unwrap_or_default()),
                },
            }
        } else {
            Engine::HalfLife
        };
        let ts = match self.ts {
            TsChoice::Keep => TsSource::Keep,
            TsChoice::Download => TsSource::Download,
            TsChoice::Local => TsSource::Local(self.ts_path.clone().unwrap_or_default()),
        };
        let libs = match (self.rust_port, self.port_from_url) {
            (false, _) => GameLibs::Original,
            (true, true) => GameLibs::RustPort(PortSource::Url(self.port_url.read(cx).value().trim().to_owned())),
            (true, false) => GameLibs::RustPort(PortSource::Dir(self.port_dir.clone().unwrap_or_default())),
        };
        Plan { root: self.root(), engine, ts, libs }
    }

    fn push_log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > LOG_LINES {
            self.log.drain(..self.log.len() - LOG_LINES);
        }
    }
}

impl Launcher {
    fn browse(&mut self, field: PathField, directories: bool, cx: &mut Context<Self>) {
        let prompt = match field {
            PathField::HalfLife | PathField::ValveFrom => "Select the Half-Life folder",
            PathField::XashRoot => "Select where to install",
            PathField::TsLocal if directories => "Select a folder containing ts",
            PathField::TsLocal => "Select ts-3-final.exe",
            PathField::PortDir => "Select the Rust port dist folder",
        };
        let rx = cx.prompt_for_paths(PathPromptOptions { files: !directories, directories, multiple: false, prompt: Some(prompt.into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else { return };
            let Some(path) = paths.pop() else { return };
            let _ = this.update(cx, |this, cx| {
                let s = &mut this.setup;
                match field {
                    PathField::HalfLife => {
                        s.half_life = Some(path);
                        s.refresh_existing();
                    }
                    PathField::XashRoot => {
                        s.xash_root = path;
                        s.refresh_existing();
                    }
                    PathField::ValveFrom => s.valve_from = Some(path),
                    PathField::TsLocal => s.ts_path = Some(path),
                    PathField::PortDir => s.port_dir = Some(path),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_install(&mut self, cx: &mut Context<Self>) {
        let plan = self.setup.plan(cx);
        if plan.validate().is_err() {
            return;
        }
        let s = &mut self.setup;
        s.steps = plan.steps();
        s.step = 0;
        s.bytes = (0, None);
        s.log.clear();
        s.error = None;
        self.screen = Screen::Installing;

        let (tx, rx) = async_channel::unbounded();
        let events = tx.clone();
        let progress = Progress::new(move |e| {
            let _ = events.send_blocking(Msg::Event(e));
        });
        self.setup.cancel = Some(progress.cancel_flag());
        std::thread::spawn(move || {
            let result = plan.run(&progress).map_err(|e| format!("{e:#}"));
            let _ = tx.send_blocking(Msg::Done(result));
        });
        cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                // Apply everything queued since the last frame in one update.
                let mut batch = vec![first];
                while let Ok(m) = rx.try_recv() {
                    batch.push(m);
                }
                if this.update(cx, |this, cx| batch.into_iter().for_each(|m| this.apply(m, cx))).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn apply(&mut self, msg: Msg, cx: &mut Context<Self>) {
        let s = &mut self.setup;
        match msg {
            Msg::Event(Event::Step { index, title, .. }) => {
                s.step = index;
                s.bytes = (0, None);
                s.push_log(format!("== {title}"));
            }
            Msg::Event(Event::Bytes { done, total }) => s.bytes = (done, total),
            Msg::Event(Event::Log(line)) => s.push_log(line),
            Msg::Done(Ok(installation)) => {
                s.cancel = None;
                let (id, saved) = self.state.update(cx, |state, cx| {
                    let id = state.store.add(installation);
                    cx.notify();
                    (id, state.save())
                });
                self.notice = saved.err().map(Err);
                self.show(Screen::Instance(id), cx);
            }
            Msg::Done(Err(e)) => {
                s.cancel = None;
                s.push_log(format!("error: {e}"));
                s.error = Some(e);
            }
        }
        cx.notify();
    }

    pub(super) fn render_welcome(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let card = |id: &'static str, icon: IconName, title: &str, text: &str, xash: bool, cx: &mut Context<Self>| {
            v_flex()
                .id(id)
                .flex_1()
                .min_w_0()
                .gap_3()
                .p_6()
                .rounded(cx.theme().radius_lg)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().secondary)
                .cursor_pointer()
                .hover(|s| s.border_color(cx.theme().primary))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.setup.use_xash = xash;
                    this.setup.refresh_existing();
                    this.show(Screen::Setup, cx);
                }))
                .child(div().text_color(cx.theme().primary).child(Icon::new(icon).large()))
                .child(div().text_lg().font_semibold().child(title.to_owned()))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child(text.to_owned()))
        };
        let has_installs = !self.state.read(cx).store.instances.is_empty();
        v_flex()
            .size_full()
            .p_8()
            .gap_6()
            .child(div().text_2xl().font_bold().child("How do you want to play?"))
            .child(
                h_flex()
                    .gap_4()
                    .items_stretch()
                    .child(card(
                        "use-half-life",
                        IconName::Folder,
                        "Use my Half-Life install",
                        "Pick an existing Half-Life folder (Steam or otherwise). TS is installed into its ts folder, \
                         or an existing ts folder is used as it is.",
                        false,
                        cx,
                    ))
                    .child(card(
                        "use-xash",
                        IconName::Globe,
                        "Install Xash3D FWGS",
                        "Download the free Xash3D FWGS engine into a folder of your choice. No Half-Life purchase needed: \
                         the Half-Life data comes from xash3d-ts, or is copied from a local install.",
                        true,
                        cx,
                    )),
            )
            .when(has_installs, |d| {
                d.child(h_flex().child(
                    Button::new("back-to-select").ghost().icon(IconName::ArrowLeft).label("Back to installations").on_click(cx.listener(
                        |this, _, _, cx| {
                            let next = match this.state.read(cx).store.instances.as_slice() {
                                [only] => Screen::Instance(only.id.clone()),
                                _ => Screen::Select,
                            };
                            this.show(next, cx);
                        },
                    )),
                ))
            })
    }

    /// What the chosen folder already has, as one line.
    fn existing_summary(&self, cx: &App) -> Div {
        let e = self.setup.existing;
        let mut found = Vec::new();
        if let Some(build) = e.xash {
            found.push(build.label().to_owned());
        }
        if e.valve {
            found.push("Half-Life data (valve)".into());
        }
        if let Some(libs) = e.ts {
            found.push(format!("TS ({})", libs.label()));
        }
        let (text, color) = if found.is_empty() {
            ("Nothing installed here yet.".to_owned(), cx.theme().muted_foreground)
        } else {
            (format!("Already here: {}", found.join(", ")), cx.theme().success)
        };
        h_flex().gap_2().text_sm().text_color(color).child(Icon::new(IconName::Info).small()).child(text)
    }

    pub(super) fn render_setup(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plan = self.setup.plan(cx);
        let problem = plan.validate().err().map(|e| format!("{e:#}"));
        let runtime = plan.runtime().ok();
        let s = &self.setup;

        let location = if s.use_xash {
            section("Install folder", "Xash3D FWGS, the Half-Life data and TS go here.", cx)
                .child(
                    h_flex().gap_2().child(path_text(Some(&s.xash_root), "", cx)).child(
                        Button::new("browse-xash")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("Browse…")
                            .on_click(cx.listener(|this, _, _, cx| this.browse(PathField::XashRoot, true, cx))),
                    ),
                )
                .child(self.existing_summary(cx))
        } else {
            let detected = s.detected.clone();
            section("Half-Life folder", "The folder that contains valve/ (and hl.exe or hl_linux).", cx)
                .child(
                    h_flex().gap_2().child(path_text(s.half_life.as_deref(), "No folder selected", cx)).child(
                        Button::new("browse-hl")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("Browse…")
                            .on_click(cx.listener(|this, _, _, cx| this.browse(PathField::HalfLife, true, cx))),
                    ),
                )
                .when(s.half_life.is_some(), |d| d.child(self.existing_summary(cx)))
                .when(!detected.is_empty(), |d| {
                    d.child(div().text_sm().text_color(cx.theme().muted_foreground).child("Found in Steam:")).children(
                        detected.into_iter().enumerate().map(|(i, p)| {
                            let label = p.display().to_string();
                            Button::new(("detected", i)).ghost().small().label(label).on_click(cx.listener(move |this, _, _, cx| {
                                this.setup.half_life = Some(p.clone());
                                this.setup.refresh_existing();
                                cx.notify();
                            }))
                        }),
                    )
                })
        };

        let engine = (s.use_xash && s.existing.xash.is_some()).then(|| {
            let build = s.existing.xash.map(|b| b.label()).unwrap_or_default();
            section("Engine", "", cx).child(
                RadioGroup::vertical("engine-source")
                    .selected_index(Some(if s.keep_engine { 0 } else { 1 }))
                    .on_change(cx.listener(|this, ix: &usize, _, cx| {
                        this.setup.keep_engine = *ix == 0;
                        cx.notify();
                    }))
                    .child(Radio::new("engine-keep").label(format!("Keep the installed {build}")))
                    .child(Radio::new("engine-download").label("Download the latest Xash3D FWGS build")),
            )
        });

        let valve = s.use_xash.then(|| {
            let choices = s.valve_choices();
            let selected = choices.iter().position(|c| *c == s.valve);
            section("Half-Life data (valve)", "Xash3D needs the Half-Life game data next to TS.", cx)
                .child(
                    RadioGroup::vertical("valve-source")
                        .selected_index(selected)
                        .on_change(cx.listener(move |this, ix: &usize, _, cx| {
                            this.setup.valve = this.setup.valve_choices()[*ix];
                            cx.notify();
                        }))
                        .children(choices.iter().map(|c| match c {
                            ValveChoice::Keep => Radio::new("valve-keep").label("Keep the existing valve folder"),
                            ValveChoice::Download => {
                                Radio::new("valve-download").label("Download it from The-Specialists-Archive/xash3d-ts (about 470 MB)")
                            }
                            ValveChoice::Copy => Radio::new("valve-copy").label("Copy it from a Half-Life install on this computer"),
                        })),
                )
                .when(s.valve == ValveChoice::Copy, |d| {
                    d.child(
                        h_flex().gap_2().child(path_text(s.valve_from.as_deref(), "No Half-Life folder selected", cx)).child(
                            Button::new("browse-valve")
                                .outline()
                                .icon(IconName::FolderOpen)
                                .label("Browse…")
                                .on_click(cx.listener(|this, _, _, cx| this.browse(PathField::ValveFrom, true, cx))),
                        ),
                    )
                    .when(!s.detected.is_empty() && s.valve_from.is_none(), |d| {
                        let first = s.detected[0].clone();
                        d.child(Button::new("valve-detected").ghost().small().label(format!("Use {}", first.display())).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.setup.valve_from = Some(first.clone());
                                cx.notify();
                            }),
                        ))
                    })
                })
        });

        let ts_choices = s.ts_choices();
        let existing_ts = s.existing.ts.map(|l| l.label()).unwrap_or_default();
        let ts = section("The Specialists 3.0", "", cx)
            .child(
                RadioGroup::vertical("ts-source")
                    .selected_index(ts_choices.iter().position(|c| *c == s.ts))
                    .on_change(cx.listener(|this, ix: &usize, _, cx| {
                        this.setup.ts = this.setup.ts_choices()[*ix];
                        cx.notify();
                    }))
                    .children(ts_choices.iter().map(|c| match c {
                        TsChoice::Keep => Radio::new("ts-keep").label(format!("Keep the existing ts folder ({existing_ts})")),
                        TsChoice::Download => Radio::new("ts-download").label(
                            "Download ts-3-final.exe (155 MB, from this project's GitHub release; falls back to the Internet Archive)",
                        ),
                        TsChoice::Local => {
                            Radio::new("ts-local").label("Use a local copy: the installer .exe, or a folder with a ts directory")
                        }
                    })),
            )
            .when(s.ts == TsChoice::Local, |d| {
                d.child(
                    h_flex()
                        .gap_2()
                        .child(path_text(s.ts_path.as_deref(), "Nothing selected", cx))
                        .child(
                            Button::new("browse-ts-file")
                                .outline()
                                .icon(IconName::File)
                                .label("Installer…")
                                .on_click(cx.listener(|this, _, _, cx| this.browse(PathField::TsLocal, false, cx))),
                        )
                        .child(
                            Button::new("browse-ts-dir")
                                .outline()
                                .icon(IconName::FolderOpen)
                                .label("Folder…")
                                .on_click(cx.listener(|this, _, _, cx| this.browse(PathField::TsLocal, true, cx))),
                        ),
                )
            });

        let keeping_ts = s.ts == TsChoice::Keep;
        let original_label = match (keeping_ts, s.existing.ts) {
            (true, Some(TsLibs::RustPort)) => "Keep the libraries in the ts folder (Rust port)",
            (true, _) => "Keep the libraries in the ts folder (TS 3.0 original)",
            (false, _) => "Original TS 3.0 client.dll / mp.dll",
        };
        let original_is_original = !(keeping_ts && s.existing.ts == Some(TsLibs::RustPort));
        let original_note = match (s.use_xash, cfg!(windows), original_is_original) {
            (true, false, true) => "Runs the 32-bit Windows Xash3D FWGS through Wine (install wine first).",
            (false, false, true) => "TS 3.0 has no Linux client: Steam must run Half-Life through Proton.",
            _ => "",
        };
        let libs = section("Game libraries", "TS 3.0 only shipped a 32-bit Windows client. The Rust port provides native libraries.", cx)
            .child(
                RadioGroup::vertical("libs")
                    .selected_index(Some(s.rust_port as usize))
                    .on_change(cx.listener(|this, ix: &usize, _, cx| {
                        this.setup.rust_port = *ix == 1;
                        cx.notify();
                    }))
                    .child(Radio::new("libs-original").label(original_label))
                    .child(Radio::new("libs-port").label("Install the Rust port libraries (native; from its dist/ folder or an archive)")),
            )
            .when(!s.rust_port && !original_note.is_empty(), |d| {
                d.child(div().text_sm().text_color(cx.theme().warning).child(original_note))
            })
            .when(s.rust_port, |d| {
                d.child(
                    RadioGroup::horizontal("port-source")
                        .selected_index(Some(s.port_from_url as usize))
                        .on_change(cx.listener(|this, ix: &usize, _, cx| {
                            this.setup.port_from_url = *ix == 1;
                            cx.notify();
                        }))
                        .child(Radio::new("port-dir").label("Local folder"))
                        .child(Radio::new("port-url").label("Download URL (.zip / .tar.gz)")),
                )
                .child(if s.port_from_url {
                    h_flex().child(Input::new(&s.port_url))
                } else {
                    h_flex().gap_2().child(path_text(s.port_dir.as_deref(), "No folder selected", cx)).child(
                        Button::new("browse-port")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("Browse…")
                            .on_click(cx.listener(|this, _, _, cx| this.browse(PathField::PortDir, true, cx))),
                    )
                })
            });

        let steps = plan.steps();
        let summary = section("Summary", "", cx)
            .when_some(runtime, |d, r| d.child(div().text_sm().child(format!("Engine: {}", r.label()))))
            .when(steps.is_empty(), |d| d.child(div().text_sm().child("Nothing to download: the existing files are used as they are.")))
            .children(steps.into_iter().enumerate().map(|(i, s)| div().text_sm().child(format!("{}. {s}", i + 1))));
        let action = if plan.steps().is_empty() { "Add installation" } else { "Install" };

        v_flex()
            .size_full()
            .child(
                v_flex()
                    .id("setup-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_6()
                    .gap_4()
                    .child(location)
                    .children(engine)
                    .children(valve)
                    .child(ts)
                    .child(libs)
                    .child(summary),
            )
            .child(
                h_flex()
                    .px_6()
                    .py_3()
                    .gap_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("setup-back")
                            .ghost()
                            .icon(IconName::ArrowLeft)
                            .label("Back")
                            .on_click(cx.listener(|this, _, _, cx| this.show(Screen::Welcome, cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .overflow_hidden()
                            .text_ellipsis()
                            .children(problem.clone()),
                    )
                    .child(
                        Button::new("install")
                            .primary()
                            .icon(IconName::ArrowRight)
                            .label(action)
                            .disabled(problem.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.start_install(cx))),
                    ),
            )
    }

    pub(super) fn render_installing(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.setup;
        let running = s.cancel.is_some();
        let (done, total) = s.bytes;
        let percent = total.filter(|t| *t > 0).map(|t| done as f32 * 100.0 / t as f32);
        let detail = match total {
            Some(t) => format!("{} of {}", human_bytes(done), human_bytes(t)),
            None if done > 0 => human_bytes(done),
            None => String::new(),
        };
        let steps = s.steps.iter().enumerate().map(|(i, title)| {
            let (icon, color) = if i < s.step || (!running && s.error.is_none()) {
                (IconName::CircleCheck, cx.theme().success)
            } else if i == s.step && s.error.is_some() {
                (IconName::CircleX, cx.theme().danger)
            } else if i == s.step {
                (IconName::ArrowRight, cx.theme().primary)
            } else {
                (IconName::Dash, cx.theme().muted_foreground)
            };
            h_flex().gap_2().child(div().text_color(color).child(icon)).child(div().text_sm().child(title.clone()))
        });
        v_flex()
            .size_full()
            .p_6()
            .gap_4()
            .child(v_flex().gap_2().children(steps))
            .when(running, |d| {
                d.child(
                    v_flex()
                        .gap_1()
                        .child(ProgressBar::new("install-progress").loading(percent.is_none()).value(percent.unwrap_or(0.0)))
                        .child(div().text_xs().text_color(cx.theme().muted_foreground).child(detail)),
                )
            })
            .when_some(s.error.clone(), |d, e| d.child(Alert::error("install-error", e).title("Install failed")))
            .child(
                v_flex()
                    .id("install-log")
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .font_family("monospace")
                    .text_xs()
                    .overflow_y_scrollbar()
                    .children(s.log.iter().map(|l| div().child(l.clone()))),
            )
            .child(
                h_flex()
                    .gap_3()
                    .justify_end()
                    .when(running, |d| {
                        d.child(Button::new("cancel").outline().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                            if let Some(flag) = &this.setup.cancel {
                                flag.store(true, Ordering::Relaxed);
                            }
                            cx.notify();
                        })))
                    })
                    .when(!running, |d| {
                        d.child(
                            Button::new("back-to-setup")
                                .ghost()
                                .icon(IconName::ArrowLeft)
                                .label("Back to setup")
                                .on_click(cx.listener(|this, _, _, cx| this.show(Screen::Setup, cx))),
                        )
                        .child(Button::new("retry").primary().label("Retry").on_click(cx.listener(|this, _, _, cx| this.start_install(cx))))
                    }),
            )
    }
}
