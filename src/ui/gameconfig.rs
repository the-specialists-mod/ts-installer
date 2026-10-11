//! The game configuration tab: TS settings in `ts/config.cfg` (and FWGS's `video.cfg`) edited
//! without starting the game, every cvar in an editable table, and `ts/userconfig.cfg`.

use std::collections::HashMap;

use gpui_kit::base::Disableable;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, NumberInput, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme, IconName, IndexPath, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use ts_launcher::gamecfg::{self, GameConfig, Kind, Section, Setting};
use ts_launcher::install::Runtime;

use super::{AppState, section};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Section(Section),
    Advanced,
}

const PAGES: [Page; 7] = [
    Page::Section(Section::Player),
    Page::Section(Section::Mouse),
    Page::Section(Section::Video),
    Page::Section(Section::Audio),
    Page::Section(Section::Hud),
    Page::Section(Section::Network),
    Page::Advanced,
];

/// The input element of one setting.
enum Widget {
    Slider(Entity<SliderState>),
    Number(Entity<InputState>),
    Text(Entity<InputState>),
    Select(Entity<SelectState<Vec<SharedString>>>),
}

/// Rows shown at most in the cvar table; the filter narrows it down.
const MAX_ROWS: usize = 150;

pub struct GameConfigTab {
    state: Entity<AppState>,
    /// The installation this tab shows.
    id: String,
    page: Page,
    cfg: Result<GameConfig, String>,
    widgets: HashMap<&'static str, Widget>,
    filter: Entity<InputState>,
    /// Value inputs of the cvar table, created as rows are shown.
    rows: HashMap<String, Entity<InputState>>,
    new_name: Entity<InputState>,
    new_value: Entity<InputState>,
    userconfig: Entity<TextareaState>,
    status: Option<Result<String, String>>,
    /// Whether the game ran at the last render, to reload what it wrote when it quits.
    was_running: bool,
}

impl GameConfigTab {
    pub fn new(state: Entity<AppState>, id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter cvars"));
        cx.subscribe(&filter, |_, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        let new_name = cx.new(|cx| InputState::new(window, cx).placeholder("cvar"));
        let new_value = cx.new(|cx| InputState::new(window, cx).placeholder("value"));
        let userconfig = cx.new(|cx| TextareaState::new(window, cx).rows(12));
        cx.subscribe(&userconfig, |this, input, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                let text = input.read(cx).value().to_string();
                if let Ok(cfg) = &mut this.cfg {
                    cfg.set_userconfig(&text);
                }
                cx.notify();
            }
        })
        .detach();
        let was_running = state.update(cx, |s, _| s.game_running());
        let mut this = Self {
            state,
            id,
            page: PAGES[0],
            cfg: Err(String::new()),
            widgets: HashMap::new(),
            filter,
            rows: HashMap::new(),
            new_name,
            new_value,
            userconfig,
            status: None,
            was_running,
        };
        this.reload(window, cx);
        this
    }

    fn runtime(&self, cx: &App) -> Option<Runtime> {
        self.state.read(cx).instance(&self.id).map(|i| i.install.runtime)
    }

    /// Reads the files again, dropping staged edits.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(inst) = self.state.read(cx).instance(&self.id).cloned() else { return };
        self.cfg = GameConfig::load(&inst.ts_dir(), inst.install.runtime).map_err(|e| format!("{e:#}"));
        let text = self.cfg.as_ref().map(|c| c.userconfig().to_owned()).unwrap_or_default();
        self.userconfig.update(cx, |s, cx| s.set_value(text, window, cx));
        self.rebuild(window, cx);
    }

    /// Recreates the inputs from the staged values (after a reload, or edits on another page).
    fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.widgets.clear();
        self.rows.clear();
        let Ok(cfg) = &self.cfg else { return };
        let cfg = cfg.clone();
        for s in gamecfg::SETTINGS.iter().filter(|s| s.supported(cfg.runtime)) {
            if let Some(w) = self.widget(s, &cfg, window, cx) {
                self.widgets.insert(s.cvar, w);
            }
        }
        cx.notify();
    }

    fn widget(&mut self, s: &'static Setting, cfg: &GameConfig, window: &mut Window, cx: &mut Context<Self>) -> Option<Widget> {
        let value = cfg.value(s);
        Some(match s.kind {
            Kind::Slider { min, max, step } => {
                let v = value.trim().parse::<f64>().unwrap_or(min).clamp(min, max);
                let slider = cx.new(|_| SliderState::new().min(min as f32).max(max as f32).step(step as f32).default_value(v as f32));
                cx.subscribe(&slider, move |this, _, e: &SliderEvent, cx| {
                    let SliderEvent::Change(v) = e else { return };
                    this.stage(s, &gamecfg::format_number(v.start() as f64, step), cx);
                })
                .detach();
                Widget::Slider(slider)
            }
            Kind::Number { min, max, step } => {
                let input = cx.new(|cx| InputState::new(window, cx).default_value(value).step(step).min(min).max(max));
                cx.subscribe(&input, move |this, input, e: &InputEvent, cx| {
                    if !matches!(e, InputEvent::Change) {
                        return;
                    }
                    let text = input.read(cx).value().trim().to_string();
                    if let Ok(v) = text.parse::<f64>() {
                        this.stage(s, &gamecfg::format_number(v, step.min(0.001)), cx);
                    }
                })
                .detach();
                Widget::Number(input)
            }
            Kind::Text => {
                let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
                cx.subscribe(&input, move |this, input, e: &InputEvent, cx| {
                    if matches!(e, InputEvent::Change) {
                        let text = input.read(cx).value().to_string();
                        this.stage(s, &text, cx);
                    }
                })
                .detach();
                Widget::Text(input)
            }
            Kind::Choice(_) | Kind::Resolution => {
                let choices = cfg.choices(s);
                let labels: Vec<SharedString> = choices.iter().map(|(_, l)| l.clone().into()).collect();
                let ix = cfg.choice_index(s);
                let select = cx.new(|cx| SelectState::new(labels, Some(IndexPath::new(ix)), window, cx));
                cx.subscribe(&select, move |this, _, e: &SelectEvent<Vec<SharedString>>, cx| {
                    let SelectEvent::Confirm(Some(label)) = e else { return };
                    let Ok(cfg) = &this.cfg else { return };
                    if let Some((v, _)) = cfg.choices(s).into_iter().find(|(_, l)| l.as_str() == label.as_ref()) {
                        this.stage(s, &v, cx);
                    }
                })
                .detach();
                Widget::Select(select)
            }
            Kind::Toggle | Kind::InvertPitch => return None,
        })
    }

    fn stage(&mut self, s: &Setting, value: &str, cx: &mut Context<Self>) {
        if let Ok(cfg) = &mut self.cfg {
            cfg.set(s, value);
        }
        self.status = None;
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.update(cx, |s, _| s.game_running()) {
            self.status = Some(Err("The game is running. Quit it first: it rewrites config.cfg when it quits.".into()));
            cx.notify();
            return;
        }
        let Ok(cfg) = &mut self.cfg else { return };
        self.status = Some(match cfg.save() {
            Ok(files) if files.is_empty() => Ok("Nothing to save".into()),
            Ok(files) => Ok(format!(
                "Saved {}",
                files.iter().filter_map(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()).collect::<Vec<_>>().join(", ")
            )),
            Err(e) => Err(format!("Could not save: {e:#}")),
        });
        self.rebuild(window, cx);
    }

    fn set_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if page != self.page {
            self.page = page;
            // Another page may have changed values these inputs show.
            self.rebuild(window, cx);
        }
    }

    fn render_setting(&self, s: &'static Setting, cfg: &GameConfig, cx: &mut Context<Self>) -> Div {
        let value = cfg.value(s);
        let control: AnyElement = match (&s.kind, self.widgets.get(s.cvar)) {
            (Kind::Toggle | Kind::InvertPitch, _) => Switch::new(SharedString::from(format!("sw-{}", s.cvar)))
                .checked(gamecfg::is_on(&value))
                .on_click(cx.listener(move |this, on: &bool, _, cx| this.stage(s, if *on { "1" } else { "0" }, cx)))
                .into_any_element(),
            (_, Some(Widget::Slider(state))) => h_flex()
                .gap_3()
                .child(div().w(px(240.)).child(Slider::new(state)))
                .child(div().text_sm().w(px(60.)).child(match s.kind {
                    Kind::Slider { step, .. } => value.trim().parse().map_or(value.clone(), |v| gamecfg::format_number(v, step)),
                    _ => value.clone(),
                }))
                .into_any_element(),
            (_, Some(Widget::Number(state))) => div().w(px(160.)).child(NumberInput::new(state).small()).into_any_element(),
            (_, Some(Widget::Text(state))) => div().w(px(260.)).child(Input::new(state).small()).into_any_element(),
            (_, Some(Widget::Select(state))) => div()
                .w(px(240.))
                .child(Select::new(state).id(SharedString::from(format!("sel-{}", s.cvar))).small().menu_max_h(px(280.)))
                .into_any_element(),
            _ => div().into_any_element(),
        };
        let muted = cx.theme().muted_foreground;
        let name = match s.kind {
            Kind::Resolution => "width, height".to_owned(),
            _ => s.cvar.to_owned(),
        };
        let default = match s.kind {
            Kind::Resolution => "desktop",
            _ => s.default_for(cfg.runtime),
        };
        let note = if cfg.is_set(s) { name } else { format!("{name} · default {default}") };
        h_flex()
            .gap_4()
            .py_1()
            .items_center()
            .child(
                v_flex()
                    .w(px(280.))
                    .flex_none()
                    .child(div().text_sm().child(s.label))
                    .child(div().text_xs().text_color(muted).child(note))
                    .when(!s.help.is_empty(), |d| d.child(div().text_xs().text_color(muted).child(s.help))),
            )
            .child(div().flex_1().min_w_0().child(control))
    }

    fn render_section(&self, sec: Section, cfg: &GameConfig, cx: &mut Context<Self>) -> Div {
        let note = match (sec, cfg.runtime) {
            (Section::Video, Runtime::GoldSrc) => {
                "Half-Life (Steam) keeps the resolution and display mode in its own settings (hl.conf on Linux, the registry on \
                 Windows), not in config.cfg. Set them in the game's Video options."
            }
            (Section::Video, Runtime::Xash(_)) => {
                "Resolution and display mode are saved in ts/video.cfg. TS fixes the field of view at 80, so there is no FOV setting."
            }
            (Section::Hud, _) => "TS 3.0 has no auto-reload option.",
            (Section::Network, _) => "Values above what the server allows are lowered by the server.",
            (Section::Player, _) => "The player model is chosen in the Player models tab.",
            _ => "",
        };
        let mut body = section(sec.title(), note, cx);
        for s in gamecfg::settings(cfg.runtime, sec) {
            body = body.child(self.render_setting(s, cfg, cx));
        }
        body
    }

    fn row_input(&mut self, name: &str, value: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(input) = self.rows.get(name) {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value.to_owned()));
        let cvar = name.to_owned();
        cx.subscribe(&input, move |this, input, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                let v = input.read(cx).value().to_string();
                if let Ok(cfg) = &mut this.cfg {
                    cfg.set_cvar(&cvar, &v);
                }
                this.status = None;
                cx.notify();
            }
        })
        .detach();
        self.rows.insert(name.to_owned(), input.clone());
        input
    }

    fn add_cvar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_name.read(cx).value().trim().to_string();
        let value = self.new_value.read(cx).value().to_string();
        if name.is_empty() || name.contains(char::is_whitespace) {
            self.status = Some(Err("Enter a cvar name without spaces".into()));
            cx.notify();
            return;
        }
        if let Ok(cfg) = &mut self.cfg {
            cfg.set_cvar(&name, &value);
        }
        self.rows.remove(&name);
        self.new_name.update(cx, |s, cx| s.set_value("", window, cx));
        self.new_value.update(cx, |s, cx| s.set_value("", window, cx));
        self.filter.update(cx, |s, cx| s.set_value(name, window, cx));
        cx.notify();
    }

    fn render_advanced(&mut self, cfg: &GameConfig, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let filter = self.filter.read(cx).value().trim().to_ascii_lowercase();
        let all = cfg.cvars();
        let matching: Vec<_> =
            all.iter().filter(|(n, v)| filter.is_empty() || n.to_ascii_lowercase().contains(&filter) || v.contains(&filter)).collect();
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let mut table = v_flex().border_1().border_color(border).rounded(cx.theme().radius).child(
            h_flex()
                .px_3()
                .py_1()
                .gap_3()
                .bg(cx.theme().muted)
                .text_xs()
                .text_color(muted)
                .child(div().w(px(260.)).child("cvar"))
                .child(div().flex_1().child("value")),
        );
        for (name, value) in matching.iter().take(MAX_ROWS) {
            let input = self.row_input(name, value, window, cx);
            let known = gamecfg::setting(name).map(|s| s.label).unwrap_or("");
            table = table.child(
                h_flex()
                    .px_3()
                    .py_0p5()
                    .gap_3()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        v_flex()
                            .w(px(260.))
                            .flex_none()
                            .child(div().text_sm().font_family("monospace").child(name.clone()))
                            .when(!known.is_empty(), |d| d.child(div().text_xs().text_color(muted).child(known))),
                    )
                    .child(div().flex_1().child(Input::new(&input).xsmall())),
            );
        }
        let shown = matching.len().min(MAX_ROWS);
        let ucfg_note = match cfg.runtime {
            Runtime::GoldSrc => "Half-Life runs ts/userconfig.cfg after config.cfg and never overwrites it",
            Runtime::Xash(_) => "Xash3D FWGS runs ts/userconfig.cfg at the end of config.cfg and never overwrites it",
        };
        v_flex()
            .gap_4()
            .child(
                section(
                    "All cvars in config.cfg",
                    "Edit any archived cvar. Cvars the game doesn't archive are dropped when it rewrites config.cfg; put those \
                     in userconfig.cfg below.",
                    cx,
                )
                .child(h_flex().gap_2().child(div().w(px(260.)).child(Input::new(&self.filter).small().cleanable(true))).child(
                    div().text_xs().text_color(muted).child(if shown < matching.len() {
                        format!("{shown} of {} shown, refine the filter", matching.len())
                    } else {
                        format!("{} of {} cvars", matching.len(), all.len())
                    }),
                ))
                .child(table)
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().text_sm().child("Add"))
                        .child(div().w(px(200.)).child(Input::new(&self.new_name).small()))
                        .child(div().w(px(200.)).child(Input::new(&self.new_value).small()))
                        .child(
                            Button::new("add-cvar")
                                .small()
                                .outline()
                                .icon(IconName::Plus)
                                .label("Add")
                                .on_click(cx.listener(|this, _, window, cx| this.add_cvar(window, cx))),
                        ),
                ),
            )
            .child(
                section("userconfig.cfg", &format!("{ucfg_note}: the place for aliases, binds with ';' and other custom commands."), cx)
                    .child(Textarea::new(&self.userconfig).h(px(220.)).font_family("monospace").text_sm()),
            )
    }
}

impl Render for GameConfigTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.state.update(cx, |s, _| s.game_running());
        if self.was_running && !running && self.cfg.as_ref().is_ok_and(|c| !c.dirty()) {
            // The game rewrote config.cfg as it quit.
            self.reload(window, cx);
        }
        self.was_running = running;
        if self.runtime(cx).is_none() {
            return div().into_any_element();
        }
        let cfg = match &self.cfg {
            Ok(cfg) => cfg.clone(),
            Err(e) => {
                return v_flex()
                    .p_6()
                    .gap_3()
                    .child(Alert::error("cfg-error", format!("Could not read the game configuration: {e}")))
                    .child(Button::new("retry").label("Try again").on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))))
                    .into_any_element();
            }
        };
        let changes = cfg.changes();
        let dirty = cfg.dirty();
        let selected = PAGES.iter().position(|p| *p == self.page).unwrap_or(0);

        let body = match self.page {
            Page::Section(sec) => self.render_section(sec, &cfg, cx),
            Page::Advanced => self.render_advanced(&cfg, window, cx),
        };
        let mut notes = v_flex().gap_2();
        if running {
            notes = notes.child(Alert::warning(
                "running",
                "The game is running. It rewrites config.cfg when it quits, so saving is off until then.",
            ));
        }
        if !cfg.config_exists() {
            let root = cfg.config_path().parent().and_then(|ts| ts.parent()).map(|r| r.to_path_buf()).unwrap_or_default();
            let from = match cfg.seed() {
                Some(p) => {
                    format!(" It will start as a copy of {}, so the key bindings stay.", p.strip_prefix(&root).unwrap_or(p).display())
                }
                None => String::new(),
            };
            notes = notes.child(Alert::info(
                "fresh",
                format!(
                    "ts/config.cfg doesn't exist yet: the game writes it when it first quits. Settings it doesn't have show \
                     their defaults. Saving creates it.{from}"
                ),
            ));
        }

        let theme = cx.theme();
        let (bottom, color) = match &self.status {
            _ if running => ("Saving is off while the game runs".to_owned(), theme.warning),
            Some(Ok(m)) if !dirty => (m.clone(), theme.success),
            Some(Err(m)) => (m.clone(), theme.danger),
            _ => (String::new(), theme.muted_foreground),
        };
        let pending = match changes.len() {
            _ if !bottom.is_empty() => bottom,
            0 if dirty => "Unsaved changes".to_owned(),
            0 => "No unsaved changes".to_owned(),
            1 => format!("1 unsaved change: {}", changes[0]),
            n if n <= 4 => format!("{n} unsaved changes: {}", changes.join(", ")),
            n => format!("{n} unsaved changes"),
        };
        v_flex()
            .size_full()
            .child(
                TabBar::new("config-pages")
                    .pill()
                    .small()
                    .px_6()
                    .pt_3()
                    .selected_index(selected)
                    .on_click(cx.listener(|this, ix: &usize, window, cx| this.set_page(PAGES[*ix], window, cx)))
                    .children(PAGES.iter().map(|p| {
                        Tab::new().label(match p {
                            Page::Section(s) => s.title(),
                            Page::Advanced => "Advanced",
                        })
                    })),
            )
            .child(
                v_flex()
                    // One scroll position per page.
                    .id(("config-scroll", selected))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .px_6()
                    .py_4()
                    .gap_4()
                    .child(notes)
                    .child(body),
            )
            .child(
                h_flex()
                    .px_6()
                    .py_3()
                    .gap_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(color)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(pending),
                    )
                    .child(
                        Button::new("revert")
                            .outline()
                            .icon(IconName::Undo2)
                            .label("Revert")
                            .tooltip("Drop unsaved changes and read the files again")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.status = None;
                                this.reload(window, cx);
                            })),
                    )
                    .child(
                        Button::new("save")
                            .primary()
                            .icon(IconName::Check)
                            .label("Save")
                            .disabled(running || !dirty)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
            .into_any_element()
    }
}
