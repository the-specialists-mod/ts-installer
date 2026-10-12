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

use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use ts_launcher::detect;
use ts_launcher::store::{Instance, Store};

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
        Self { state, screen, setup, instance: None, notice }
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
