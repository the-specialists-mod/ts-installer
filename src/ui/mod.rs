//! The launcher window: setup, the installation selector, and the per-installation view.

mod controls;
mod gameconfig;
mod instance;
mod models;
mod select;
mod servers;
mod setup;
pub mod theme;

use std::path::Path;
use std::process::Child;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use ts_launcher::detect;
use ts_launcher::progress::{Event, Progress};
use ts_launcher::store::{Instance, Store};
use ts_launcher::update::{self, Update};

use instance::{InstanceEvent, InstanceView};
pub use setup::Setup;

/// State shared by every view: the saved installations and the running game.
pub struct AppState {
    pub store: Store,
    game: Option<Child>,
}

impl AppState {
    pub fn instance(&self, id: &str) -> Option<&Instance> {
        self.store.get(id)
    }

    /// Saves the store, as a message for the UI on failure.
    pub fn save(&self) -> Result<(), String> {
        self.store.save().map_err(|e| format!("could not save settings: {e:#}"))
    }

    /// Whether a game this launcher started is still running.
    pub fn game_running(&mut self) -> bool {
        match &mut self.game {
            Some(child) => match child.try_wait() {
                Ok(None) => true,
                _ => {
                    self.game = None;
                    false
                }
            },
            None => false,
        }
    }

    /// Starts installation `id` with `extra` arguments after its own launch options.
    pub fn launch(&mut self, id: &str, extra: &[String]) -> Result<String, String> {
        let inst = self.store.get(id).ok_or("unknown installation")?.clone();
        let child = ts_launcher::launch::launch(&inst.install, &inst.launch, extra).map_err(|e| format!("{e:#}"))?;
        self.started(id, child)
    }

    /// Starts installation `id` as its Play button does, hosting its local server if that is on.
    pub fn play(&mut self, id: &str) -> Result<String, String> {
        let inst = self.store.get(id).ok_or("unknown installation")?.clone();
        let child = ts_launcher::launch::play(&inst.install, &inst.launch).map_err(|e| format!("{e:#}"))?;
        self.started(id, child)
    }

    fn started(&mut self, id: &str, child: Child) -> Result<String, String> {
        let msg = format!("Started (pid {})", child.id());
        self.game = Some(child);
        if let Some(i) = self.store.get_mut(id) {
            i.last_played = Some(ts_launcher::store::now());
        }
        self.store.last_used = Some(id.to_owned());
        self.save()?;
        Ok(msg)
    }
}

#[derive(Clone, PartialEq)]
enum Screen {
    Welcome,
    Setup,
    Installing,
    Select,
    Instance(String),
}

pub struct Launcher {
    state: Entity<AppState>,
    screen: Screen,
    setup: Setup,
    instance: Option<Entity<InstanceView>>,
    /// Shown above the selector or the instance view once.
    notice: Option<Result<String, String>>,
    update: UpdateState,
}

/// The launcher's own update, offered in a banner under the header.
enum UpdateState {
    Hidden,
    /// A newer release: ask whether to download it.
    Available(Update),
    Installing {
        tag: String,
        bytes: (u64, Option<u64>),
    },
    /// In place of the running binary; it starts on restart.
    Installed {
        tag: String,
        exe: std::path::PathBuf,
    },
    Failed(String),
}

impl Launcher {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut store = Store::load();
        let detected = detect::half_life_installs();
        let mut notice = None;
        // First start with TS already in a Half-Life install: register it and skip setup.
        if store.instances.is_empty()
            && let Some(inst) = detect::existing_ts_install(&detected)
        {
            notice = Some(Ok(format!("Found TS in {}", inst.root.display())));
            store.add(inst);
            if let Err(e) = store.save() {
                notice = Some(Err(format!("could not save settings: {e:#}")));
            }
        }
        let screen = match store.instances.len() {
            0 => Screen::Welcome,
            1 => Screen::Instance(store.instances[0].id.clone()),
            _ => Screen::Select,
        };
        let state = cx.new(|_| AppState { store, game: None });
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let setup = Setup::new(detected, window, cx);
        if !update::disabled() {
            cx.spawn(async move |this, cx| {
                let found = cx.background_executor().spawn(async { update::check() }).await;
                match found {
                    Ok(Some(u)) => {
                        let _ = this.update(cx, |this, cx| {
                            this.update = UpdateState::Available(u);
                            cx.notify();
                        });
                    }
                    Ok(None) => {}
                    Err(e) => log::warn!("checking for a launcher update: {e:#}"),
                }
            })
            .detach();
        }
        Self { state, screen, setup, instance: None, notice, update: UpdateState::Hidden }
    }

    fn install_update(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Available(u) = &self.update else { return };
        let u = u.clone();
        self.update = UpdateState::Installing { tag: u.tag.clone(), bytes: (0, None) };
        let (tx, rx) = async_channel::unbounded();
        let events = tx.clone();
        let progress = Progress::new(move |e| {
            if let Event::Bytes { done, total } = e {
                let _ = events.send_blocking(Ok((done, total)));
            }
        });
        let done = tx;
        std::thread::spawn(move || {
            let result = update::install(&u, &progress).map_err(|e| format!("{e:#}"));
            let _ = done.send_blocking(Err(result.map(|exe| (u.tag, exe))));
        });
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let finished = msg.is_err();
                let _ = this.update(cx, |this, cx| {
                    this.update = match (msg, &this.update) {
                        (Ok(bytes), UpdateState::Installing { tag, .. }) => UpdateState::Installing { tag: tag.clone(), bytes },
                        (Ok(_), _) => return,
                        (Err(Ok((tag, exe))), _) => UpdateState::Installed { tag, exe },
                        (Err(Err(e)), _) => UpdateState::Failed(e),
                    };
                    cx.notify();
                });
                if finished {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Starts the updated launcher and quits this one.
    fn restart(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Installed { exe, .. } = &self.update else { return };
        match std::process::Command::new(exe).spawn() {
            Ok(_) => cx.quit(),
            Err(e) => self.update = UpdateState::Failed(format!("start {}: {e}", exe.display())),
        }
        cx.notify();
    }

    fn render_update(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let later = |cx: &mut Context<Self>| {
            Button::new("update-later").ghost().small().label("Later").on_click(cx.listener(|this, _, _, cx| {
                this.update = UpdateState::Hidden;
                cx.notify();
            }))
        };
        let row = h_flex().px_6().py_2().gap_3().bg(theme::panel()).border_b_1().border_color(cx.theme().border).text_sm();
        Some(match &self.update {
            UpdateState::Hidden => return None,
            UpdateState::Available(u) => {
                let page = u.page.clone();
                row.child(div().text_color(cx.theme().primary).child(gpui_kit::component::Icon::new(IconName::ArrowDown)))
                    .child(div().flex_1().child(format!(
                        "Launcher {} is available (you have {}). Download it now?",
                        u.tag,
                        update::current()
                    )))
                    .when(!page.is_empty(), |r| {
                        r.child(
                            Button::new("update-notes").ghost().small().label("What's new").on_click(move |_, _, cx| cx.open_url(&page)),
                        )
                    })
                    .child(later(cx))
                    .child(
                        Button::new("update-install")
                            .primary()
                            .small()
                            .icon(IconName::ArrowDown)
                            .label("Download and install")
                            .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                    )
                    .into_any_element()
            }
            UpdateState::Installing { tag, bytes } => {
                let percent = bytes.1.filter(|t| *t > 0).map(|t| bytes.0 as f32 / t as f32 * 100.);
                row.child(div().child(format!("Downloading launcher {tag}…")))
                    .child(
                        div().flex_1().child(ProgressBar::new("update-progress").loading(percent.is_none()).value(percent.unwrap_or(0.))),
                    )
                    .into_any_element()
            }
            UpdateState::Installed { tag, .. } => row
                .child(div().flex_1().child(format!("Launcher {tag} is installed. Restart to use it.")))
                .child(later(cx))
                .child(
                    Button::new("update-restart")
                        .primary()
                        .small()
                        .icon(IconName::Redo)
                        .label("Restart now")
                        .on_click(cx.listener(|this, _, _, cx| this.restart(cx))),
                )
                .into_any_element(),
            UpdateState::Failed(e) => row
                .child(div().flex_1().text_color(cx.theme().danger).child(format!("The launcher update failed: {e}")))
                .child(div().text_color(muted).child(format!("Download it from github.com/{}/releases", update::REPO)))
                .child(later(cx))
                .into_any_element(),
        })
    }

    fn show(&mut self, screen: Screen, cx: &mut Context<Self>) {
        if !matches!(screen, Screen::Instance(_)) {
            self.instance = None;
        }
        self.screen = screen;
        cx.notify();
    }

    /// The view for installation `id`, created on first use.
    fn instance_view(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<InstanceView> {
        if let Some(view) = &self.instance
            && view.read(cx).id() == id
        {
            return view.clone();
        }
        let state = self.state.clone();
        let view = cx.new(|cx| InstanceView::new(state, id.to_owned(), window, cx));
        cx.subscribe(&view, |this, _, event: &InstanceEvent, cx| match event {
            InstanceEvent::Switch => this.show(Screen::Select, cx),
            InstanceEvent::AddInstallation => this.show(Screen::Welcome, cx),
            InstanceEvent::Removed => {
                let next = if this.state.read(cx).store.instances.is_empty() { Screen::Welcome } else { Screen::Select };
                this.show(next, cx);
            }
        })
        .detach();
        self.instance = Some(view.clone());
        view
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Screen::Instance(id) = &self.screen
            && self.state.read(cx).instance(id).is_none()
        {
            self.screen = Screen::Welcome;
        }
        let body = match self.screen.clone() {
            Screen::Welcome => self.render_welcome(cx).into_any_element(),
            Screen::Setup => self.render_setup(window, cx).into_any_element(),
            Screen::Installing => self.render_installing(cx).into_any_element(),
            Screen::Select => self.render_select(cx).into_any_element(),
            Screen::Instance(id) => self.instance_view(&id, window, cx).into_any_element(),
        };
        // The site's darkened grunge texture behind everything.
        div()
            .size_full()
            .relative()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(img(theme::GRUNGE.clone()).absolute().size_full().object_fit(ObjectFit::Cover))
            .child(div().absolute().size_full().bg(hsla(0., 0., 0., 0.5)))
            .child(
                v_flex()
                    .relative()
                    .size_full()
                    .child(
                        h_flex()
                            .px_6()
                            .py_2()
                            .gap_4()
                            .bg(hsla(0., 0., 0., 0.35))
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(img(theme::LOGO.clone()).w(px(267.)).h(px(52.)))
                            .child(theme::heading("3.0 Launcher", 22.).text_color(cx.theme().muted_foreground)),
                    )
                    .children(self.render_update(cx))
                    .child(div().flex_1().min_h_0().child(body)),
            )
    }
}

/// Section with a heading and a short explanation.
pub fn section(title: &str, note: &str, cx: &App) -> Div {
    v_flex()
        .gap_2()
        .p_4()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .bg(theme::panel())
        .child(theme::heading(title, 18.))
        .when(!note.is_empty(), |d| d.child(div().text_sm().text_color(cx.theme().muted_foreground).child(note.to_owned())))
}

pub fn path_text(path: Option<&Path>, placeholder: &str, cx: &App) -> Div {
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

/// A label/value row for detail lists.
pub fn detail_row(label: &str, value: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .gap_3()
        .text_sm()
        .child(div().w(px(140.)).flex_none().text_color(cx.theme().muted_foreground).child(label.to_owned()))
        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().child(value.into()))
}

pub fn human_bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n => format!("{:.0} KB", n as f64 / 1024.0),
    }
}

/// A Unix time as local date and time.
pub fn human_time(secs: u64) -> String {
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}
