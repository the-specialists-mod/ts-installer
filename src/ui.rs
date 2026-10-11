//! The launcher window: first-run setup, install progress, and the play screen.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::base::Disableable;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use ts_launcher::config::Config;
use ts_launcher::detect;
use ts_launcher::install::{Engine, GameLibs, Installation, Plan, PortSource, Runtime, TsSource, ValveSource};
use ts_launcher::progress::{Event, Progress};

const LOG_LINES: usize = 300;

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Welcome,
    Setup,
    Installing,
    Play,
}

/// Which path a file dialog fills in.
#[derive(Clone, Copy)]
enum PathField {
    HalfLife,
    XashRoot,
    ValveFrom,
    TsLocal,
    PortDir,
}

enum Msg {
    Event(Event),
    Done(Result<Installation, String>),
}

pub struct Launcher {
    config: Config,
    screen: Screen,

    // Setup choices.
    use_xash: bool,
    half_life: Option<PathBuf>,
    detected: Vec<PathBuf>,
    xash_root: PathBuf,
    valve_copy: bool,
    valve_from: Option<PathBuf>,
    ts_local: bool,
    ts_path: Option<PathBuf>,
    rust_port: bool,
    port_from_url: bool,
    port_dir: Option<PathBuf>,
    port_url: Entity<InputState>,

    // Install progress.
    steps: Vec<String>,
    step: usize,
    bytes: (u64, Option<u64>),
    log: Vec<String>,
    error: Option<String>,
    cancel: Option<Arc<AtomicBool>>,

    // Play screen.
    extra_args: Entity<InputState>,
    wine: Entity<InputState>,
    wine_prefix: Entity<InputState>,
    status: Option<Result<String, String>>,
}

impl Launcher {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut config = Config::load();
        let detected = detect::half_life_installs();
        // First start with TS already in a Half-Life install: skip setup and use it.
        let mut status = None;
        if config.installation.is_none()
            && let Some(inst) = detect::existing_ts_install(&detected)
        {
            status = Some(Ok(format!("Found TS in {}", inst.root.display())));
            config.installation = Some(inst);
            if let Err(e) = config.save() {
                status = Some(Err(format!("could not save settings: {e:#}")));
            }
        }
        let input = |value: &str, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            let state = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_owned()).default_value(value.to_owned()));
            cx.subscribe(&state, |this, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    this.save_launch_options(cx);
                }
            })
            .detach();
            state
        };
        let extra_args = input(&config.launch.extra_args, "e.g. +connect 1.2.3.4:27015 -dev 2", window, cx);
        let wine = input(&config.launch.wine, "wine", window, cx);
        let wine_prefix = input(&config.launch.wine_prefix, "default (~/.wine)", window, cx);
        let port_url = cx.new(|cx| InputState::new(window, cx).placeholder("https://…/ts-rust-dist.zip"));
        cx.subscribe(&port_url, |_, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        Self {
            screen: if config.installation.is_some() { Screen::Play } else { Screen::Welcome },
            config,
            use_xash: false,
            half_life: detected.first().cloned(),
            detected,
            xash_root: detect::default_xash_dir(),
            valve_copy: false,
            valve_from: None,
            ts_local: false,
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
            extra_args,
            wine,
            wine_prefix,
            status,
        }
    }

    fn plan(&self, cx: &App) -> Plan {
        let engine = if self.use_xash {
            Engine::Xash {
                valve: match (&self.valve_copy, &self.valve_from) {
                    (true, Some(p)) => ValveSource::CopyFrom(p.clone()),
                    (true, None) => ValveSource::CopyFrom(PathBuf::new()),
                    (false, _) => ValveSource::XashTs,
                },
            }
        } else {
            Engine::HalfLife
        };
        let root = if self.use_xash { self.xash_root.clone() } else { self.half_life.clone().unwrap_or_default() };
        let ts = match (self.ts_local, &self.ts_path) {
            (true, p) => TsSource::Local(p.clone().unwrap_or_default()),
            (false, _) => TsSource::Download,
        };
        let libs = match (self.rust_port, self.port_from_url) {
            (false, _) => GameLibs::Original,
            (true, true) => GameLibs::RustPort(PortSource::Url(self.port_url.read(cx).value().trim().to_owned())),
            (true, false) => GameLibs::RustPort(PortSource::Dir(self.port_dir.clone().unwrap_or_default())),
        };
        Plan { root, engine, ts, libs }
    }

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
                match field {
                    PathField::HalfLife => this.half_life = Some(path),
                    PathField::XashRoot => this.xash_root = path,
                    PathField::ValveFrom => this.valve_from = Some(path),
                    PathField::TsLocal => this.ts_path = Some(path),
                    PathField::PortDir => this.port_dir = Some(path),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_install(&mut self, cx: &mut Context<Self>) {
        let plan = self.plan(cx);
        if plan.validate().is_err() {
            return;
        }
        self.steps = plan.steps();
        self.step = 0;
        self.bytes = (0, None);
        self.log.clear();
        self.error = None;
        self.screen = Screen::Installing;

        let (tx, rx) = async_channel::unbounded();
        let events = tx.clone();
        let progress = Progress::new(move |e| {
            let _ = events.send_blocking(Msg::Event(e));
        });
        self.cancel = Some(progress.cancel_flag());
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
        match msg {
            Msg::Event(Event::Step { index, title, .. }) => {
                self.step = index;
                self.bytes = (0, None);
                self.push_log(format!("== {title}"));
            }
            Msg::Event(Event::Bytes { done, total }) => self.bytes = (done, total),
            Msg::Event(Event::Log(line)) => self.push_log(line),
            Msg::Done(Ok(installation)) => {
                self.cancel = None;
                self.config.installation = Some(installation);
                self.status = self.config.save().err().map(|e| Err(format!("could not save settings: {e:#}")));
                self.screen = Screen::Play;
            }
            Msg::Done(Err(e)) => {
                self.cancel = None;
                self.push_log(format!("error: {e}"));
                self.error = Some(e);
            }
        }
        cx.notify();
    }

    fn push_log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > LOG_LINES {
            self.log.drain(..self.log.len() - LOG_LINES);
        }
    }

    fn save_launch_options(&mut self, cx: &mut Context<Self>) {
        self.config.launch.extra_args = self.extra_args.read(cx).value().to_string();
        self.config.launch.wine = self.wine.read(cx).value().to_string();
        self.config.launch.wine_prefix = self.wine_prefix.read(cx).value().to_string();
        if let Err(e) = self.config.save() {
            self.status = Some(Err(format!("could not save settings: {e:#}")));
        }
        cx.notify();
    }

    fn play(&mut self, cx: &mut Context<Self>) {
        self.save_launch_options(cx);
        let Some(inst) = &self.config.installation else { return };
        self.status = Some(match ts_launcher::launch::launch(inst, &self.config.launch) {
            Ok(child) => Ok(format!("Started (pid {})", child.id())),
            Err(e) => Err(format!("{e:#}")),
        });
        cx.notify();
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.screen {
            Screen::Welcome => self.render_welcome(cx).into_any_element(),
            Screen::Setup => self.render_setup(window, cx).into_any_element(),
            Screen::Installing => self.render_installing(cx).into_any_element(),
            Screen::Play => self.render_play(cx).into_any_element(),
        };
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                h_flex()
                    .px_6()
                    .py_4()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().text_xl().font_bold().child("The Specialists 3.0"))
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Launcher")),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}

/// Section with a heading and a short explanation.
fn section(title: &str, note: &str, cx: &App) -> Div {
    v_flex()
        .gap_2()
        .p_4()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .child(div().font_semibold().child(title.to_owned()))
        .when(!note.is_empty(), |d| d.child(div().text_sm().text_color(cx.theme().muted_foreground).child(note.to_owned())))
}

fn path_text(path: Option<&Path>, placeholder: &str, cx: &App) -> Div {
    let (text, color) = match path {
        Some(p) if !p.as_os_str().is_empty() => (p.display().to_string(), cx.theme().foreground),
        _ => (placeholder.to_owned(), cx.theme().muted_foreground),
    };
    div()
        .flex_1()
        .min_w_0()
        .px_3()
        .py_1()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .text_sm()
        .text_color(color)
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .child(text)
}

fn human_bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n => format!("{:.0} KB", n as f64 / 1024.0),
    }
}

impl Launcher {
    fn render_welcome(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
                    this.use_xash = xash;
                    this.screen = Screen::Setup;
                    cx.notify();
                }))
                .child(div().text_color(cx.theme().primary).child(Icon::new(icon).large()))
                .child(div().text_lg().font_semibold().child(title.to_owned()))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child(text.to_owned()))
        };
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
                        "Pick an existing Half-Life folder (Steam or otherwise). TS is installed into its ts folder.",
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
            .when(self.config.installation.is_some(), |d| {
                d.child(h_flex().child(Button::new("back-to-play").ghost().label("Back to the launcher").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.screen = Screen::Play;
                        cx.notify();
                    },
                ))))
            })
    }

    fn render_setup(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plan = self.plan(cx);
        let problem = plan.validate().err().map(|e| format!("{e:#}"));
        let runtime = plan.runtime().ok();

        let location = if self.use_xash {
            section("Install folder", "Xash3D FWGS, the Half-Life data and TS go here.", cx).child(
                h_flex().gap_2().child(path_text(Some(&self.xash_root), "", cx)).child(
                    Button::new("browse-xash")
                        .outline()
                        .icon(IconName::FolderOpen)
                        .label("Browse…")
                        .on_click(cx.listener(|this, _, _, cx| this.browse(PathField::XashRoot, true, cx))),
                ),
            )
        } else {
            let detected = self.detected.clone();
            section("Half-Life folder", "The folder that contains valve/ (and hl.exe or hl_linux).", cx)
                .child(
                    h_flex().gap_2().child(path_text(self.half_life.as_deref(), "No folder selected", cx)).child(
                        Button::new("browse-hl")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("Browse…")
                            .on_click(cx.listener(|this, _, _, cx| this.browse(PathField::HalfLife, true, cx))),
                    ),
                )
                .when(!detected.is_empty(), |d| {
                    d.child(div().text_sm().text_color(cx.theme().muted_foreground).child("Found in Steam:")).children(
                        detected.into_iter().enumerate().map(|(i, p)| {
                            let label = p.display().to_string();
                            Button::new(("detected", i)).ghost().small().label(label).on_click(cx.listener(move |this, _, _, cx| {
                                this.half_life = Some(p.clone());
                                cx.notify();
                            }))
                        }),
                    )
                })
        };

        let valve = self.use_xash.then(|| {
            section("Half-Life data (valve)", "Xash3D needs the Half-Life game data next to TS.", cx)
                .child(
                    RadioGroup::vertical("valve-source")
                        .selected_index(Some(self.valve_copy as usize))
                        .on_change(cx.listener(|this, ix: &usize, _, cx| {
                            this.valve_copy = *ix == 1;
                            cx.notify();
                        }))
                        .child(Radio::new("valve-download").label("Download it from The-Specialists-Archive/xash3d-ts (about 470 MB)"))
                        .child(Radio::new("valve-copy").label("Copy it from a Half-Life install on this computer")),
                )
                .when(self.valve_copy, |d| {
                    d.child(
                        h_flex().gap_2().child(path_text(self.valve_from.as_deref(), "No Half-Life folder selected", cx)).child(
                            Button::new("browse-valve")
                                .outline()
                                .icon(IconName::FolderOpen)
                                .label("Browse…")
                                .on_click(cx.listener(|this, _, _, cx| this.browse(PathField::ValveFrom, true, cx))),
                        ),
                    )
                    .when(!self.detected.is_empty() && self.valve_from.is_none(), |d| {
                        let first = self.detected[0].clone();
                        d.child(Button::new("valve-detected").ghost().small().label(format!("Use {}", first.display())).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.valve_from = Some(first.clone());
                                cx.notify();
                            }),
                        ))
                    })
                })
        });

        let ts =
            section("The Specialists 3.0", "", cx)
                .child(
                    RadioGroup::vertical("ts-source")
                        .selected_index(Some(self.ts_local as usize))
                        .on_change(cx.listener(|this, ix: &usize, _, cx| {
                            this.ts_local = *ix == 1;
                            cx.notify();
                        }))
                        .child(Radio::new("ts-download").label(
                            "Download ts-3-final.exe (155 MB, from this project's GitHub release; falls back to the Internet Archive)",
                        ))
                        .child(Radio::new("ts-local").label("Use a local copy: the installer .exe, or a folder with a ts directory")),
                )
                .when(self.ts_local, |d| {
                    d.child(
                        h_flex()
                            .gap_2()
                            .child(path_text(self.ts_path.as_deref(), "Nothing selected", cx))
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

        let original_note = match (self.use_xash, cfg!(windows)) {
            (true, false) => "Runs the 32-bit Windows Xash3D FWGS through Wine (install wine first).",
            (false, false) => "TS 3.0 has no Linux client: Steam must run Half-Life through Proton.",
            _ => "",
        };
        let libs = section("Game libraries", "TS 3.0 only shipped a 32-bit Windows client. The Rust port provides native libraries.", cx)
            .child(
                RadioGroup::vertical("libs")
                    .selected_index(Some(self.rust_port as usize))
                    .on_change(cx.listener(|this, ix: &usize, _, cx| {
                        this.rust_port = *ix == 1;
                        cx.notify();
                    }))
                    .child(Radio::new("libs-original").label("Original TS 3.0 client.dll / mp.dll"))
                    .child(Radio::new("libs-port").label("Rust port (native, from its dist/ folder or an archive)")),
            )
            .when(!self.rust_port && !original_note.is_empty(), |d| {
                d.child(div().text_sm().text_color(cx.theme().warning).child(original_note))
            })
            .when(self.rust_port, |d| {
                d.child(
                    RadioGroup::horizontal("port-source")
                        .selected_index(Some(self.port_from_url as usize))
                        .on_change(cx.listener(|this, ix: &usize, _, cx| {
                            this.port_from_url = *ix == 1;
                            cx.notify();
                        }))
                        .child(Radio::new("port-dir").label("Local folder"))
                        .child(Radio::new("port-url").label("Download URL (.zip / .tar.gz)")),
                )
                .child(if self.port_from_url {
                    h_flex().child(Input::new(&self.port_url))
                } else {
                    h_flex().gap_2().child(path_text(self.port_dir.as_deref(), "No folder selected", cx)).child(
                        Button::new("browse-port")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("Browse…")
                            .on_click(cx.listener(|this, _, _, cx| this.browse(PathField::PortDir, true, cx))),
                    )
                })
            });

        let summary = section("Summary", "", cx)
            .when_some(runtime, |d, r| d.child(div().text_sm().child(format!("Engine: {}", r.label()))))
            .children(plan.steps().into_iter().enumerate().map(|(i, s)| div().text_sm().child(format!("{}. {s}", i + 1))));

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
                    .child(Button::new("setup-back").ghost().icon(IconName::ArrowLeft).label("Back").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.screen = Screen::Welcome;
                            cx.notify();
                        },
                    )))
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
                            .label("Install")
                            .disabled(problem.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.start_install(cx))),
                    ),
            )
    }

    fn render_installing(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.cancel.is_some();
        let (done, total) = self.bytes;
        let percent = total.filter(|t| *t > 0).map(|t| done as f32 * 100.0 / t as f32);
        let detail = match total {
            Some(t) => format!("{} of {}", human_bytes(done), human_bytes(t)),
            None if done > 0 => human_bytes(done),
            None => String::new(),
        };
        let steps = self.steps.iter().enumerate().map(|(i, title)| {
            let (icon, color) = if i < self.step || (!running && self.error.is_none()) {
                (IconName::CircleCheck, cx.theme().success)
            } else if i == self.step && self.error.is_some() {
                (IconName::CircleX, cx.theme().danger)
            } else if i == self.step {
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
            .when_some(self.error.clone(), |d, e| d.child(Alert::error("install-error", e).title("Install failed")))
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
                    .children(self.log.iter().map(|l| div().child(l.clone()))),
            )
            .child(
                h_flex()
                    .gap_3()
                    .justify_end()
                    .when(running, |d| {
                        d.child(Button::new("cancel").outline().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                            if let Some(flag) = &this.cancel {
                                flag.store(true, Ordering::Relaxed);
                            }
                            cx.notify();
                        })))
                    })
                    .when(!running, |d| {
                        d.child(Button::new("back-to-setup").ghost().icon(IconName::ArrowLeft).label("Back to setup").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.screen = Screen::Setup;
                                cx.notify();
                            }),
                        ))
                        .child(Button::new("retry").primary().label("Retry").on_click(cx.listener(|this, _, _, cx| this.start_install(cx))))
                    }),
            )
    }

    fn render_play(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(inst) = self.config.installation.clone() else {
            return div().into_any_element();
        };
        let needs_wine = matches!(inst.runtime, Runtime::Xash(b) if b.needs_wine());
        let launch = &self.config.launch;
        let command = ts_launcher::launch::command(&inst, launch).map(|c| {
            std::iter::once(c.get_program()).chain(c.get_args()).map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
        });
        v_flex()
            .id("play-scroll")
            .size_full()
            .overflow_y_scrollbar()
            .p_6()
            .gap_4()
            .child(
                section("Installed", "", cx)
                    .child(div().text_sm().child(format!("Folder: {}", inst.root.display())))
                    .child(div().text_sm().child(format!("Engine: {}", inst.runtime.label()))),
            )
            .child(
                section("Launch options", "", cx)
                    .child(
                        h_flex()
                            .gap_6()
                            .child(Checkbox::new("windowed").label("Windowed").checked(launch.windowed).on_click(cx.listener(
                                |this, checked: &bool, _, cx| {
                                    this.config.launch.windowed = *checked;
                                    this.save_launch_options(cx);
                                },
                            )))
                            .child(Checkbox::new("console").label("Developer console").checked(launch.console).on_click(cx.listener(
                                |this, checked: &bool, _, cx| {
                                    this.config.launch.console = *checked;
                                    this.save_launch_options(cx);
                                },
                            ))),
                    )
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
            )
            .when_some(self.status.clone(), |d, s| {
                d.child(match s {
                    Ok(m) => Alert::success("play-status", m).into_any_element(),
                    Err(e) => Alert::error("play-status", e).into_any_element(),
                })
            })
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        Button::new("play")
                            .primary()
                            .large()
                            .icon(IconName::Play)
                            .label("Play")
                            .on_click(cx.listener(|this, _, _, cx| this.play(cx))),
                    )
                    .child(Button::new("open-folder").outline().icon(IconName::FolderOpen).label("Open folder").on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(inst) = &this.config.installation {
                                cx.open_with_system(&inst.root);
                            }
                        },
                    )))
                    .child(div().flex_1())
                    .child(Button::new("reinstall").ghost().label("Change install…").on_click(cx.listener(|this, _, _, cx| {
                        this.status = None;
                        this.screen = Screen::Welcome;
                        cx.notify();
                    }))),
            )
            .into_any_element()
    }
}
