//! The controls tab: key bindings from `ts/config.cfg`, grouped like the game's own key list.

use std::path::PathBuf;
use std::time::SystemTime;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::Disableable;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use ts_launcher::cfgfile::CfgFile;
use ts_launcher::controls::{self, Section, key_label};

use super::{AppState, section};

/// Keys shown per action; more stay bound but aren't shown.
const SLOTS: usize = 2;

pub struct ControlsTab {
    state: Entity<AppState>,
    path: PathBuf,
    sections: Vec<Section>,
    defaults: Vec<(String, String)>,
    /// What Revert goes back to: the file on disk, or the defaults when there is none.
    base: CfgFile,
    staged: CfgFile,
    /// No config.cfg on disk yet.
    fresh: bool,
    /// Modification time of config.cfg when it was read, to pick up the game's rewrite on quit.
    mtime: Option<SystemTime>,
    /// The action and slot waiting for a key.
    capture: Option<(String, usize)>,
    /// Caps Lock state when capturing started; a change means Caps Lock was pressed.
    capslock: bool,
    focus: FocusHandle,
    notice: Option<Result<String, String>>,
    confirm_reset: bool,
    _intercept: Subscription,
}

impl ControlsTab {
    pub fn new(state: Entity<AppState>, id: String, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root = state.read(cx).instance(&id).map(|i| i.install.root.clone()).unwrap_or_default();
        let path = root.join("ts/config.cfg");
        // Keystrokes are taken before key bindings (Tab moves focus otherwise) while capturing.
        let this = cx.entity().downgrade();
        let intercept = cx.intercept_keystrokes(move |e, _, cx| {
            let Some(this) = this.upgrade() else { return };
            if this.read(cx).capture.is_some() {
                let k = &e.keystroke;
                // A modifier held down was pressed first; when the platform didn't report it as a
                // modifier change (Xvfb doesn't), it is the key meant.
                let key = match k.modifiers {
                    m if m.control => "control".to_owned(),
                    m if m.alt => "alt".to_owned(),
                    m if m.shift => "shift".to_owned(),
                    _ => k.key.clone(),
                };
                this.update(cx, |this, cx| this.key_pressed(&key, cx));
                cx.stop_propagation();
            }
        });
        let mut tab = Self {
            state,
            sections: controls::load_actions(&root),
            defaults: controls::default_binds(&root),
            base: CfgFile::default(),
            staged: CfgFile::default(),
            path,
            fresh: false,
            mtime: None,
            capture: None,
            capslock: false,
            focus: cx.focus_handle(),
            notice: None,
            confirm_reset: false,
            _intercept: intercept,
        };
        tab.load();
        tab
    }

    /// (Re)reads config.cfg, dropping staged changes.
    fn load(&mut self) {
        self.fresh = !self.path.exists();
        self.mtime = self.modified();
        match CfgFile::load(&self.path) {
            Ok(cfg) if self.fresh => {
                self.base = controls::default_config(&cfg.path, &self.defaults);
            }
            Ok(cfg) => self.base = cfg,
            Err(e) => {
                self.notice = Some(Err(format!("{e:#}")));
                self.base = CfgFile::parse(&self.path, "");
            }
        }
        self.staged = self.base.clone();
        self.capture = None;
        self.confirm_reset = false;
    }

    fn modified(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.path).and_then(|m| m.modified()).ok()
    }

    fn dirty(&self) -> bool {
        self.fresh || self.staged.text() != self.base.text()
    }

    fn running(&self, cx: &mut Context<Self>) -> bool {
        self.state.update(cx, |s, _| s.game_running())
    }

    /// The description of `command`, or the command itself if the key list doesn't have it.
    fn describe(&self, command: &str) -> String {
        self.sections
            .iter()
            .flat_map(|s| &s.actions)
            .find(|a| a.command.eq_ignore_ascii_case(command))
            .map(|a| capitalize(&a.description))
            .unwrap_or_else(|| command.to_owned())
    }

    fn start_capture(&mut self, command: String, slot: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.capture = Some((command, slot));
        self.capslock = window.capslock().on;
        self.notice = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// A key press while capturing, as GPUI names it.
    fn key_pressed(&mut self, key: &str, cx: &mut Context<Self>) {
        match key {
            "escape" => self.capture = None,
            "backspace" | "delete" => {
                if let Some((command, slot)) = self.capture.take() {
                    controls::clear_slot(&mut self.staged, &command, slot);
                }
            }
            _ => match controls::key_from_gpui(key) {
                Some(name) => self.assign(name),
                None => self.notice = Some(Err(format!("The game can't bind “{key}”. Press another key, or Esc to cancel."))),
            },
        }
        cx.notify();
    }

    /// Binds the captured slot to `key` (an engine key name).
    fn assign(&mut self, key: String) {
        let Some((command, slot)) = self.capture.take() else { return };
        let stolen = controls::rebind(&mut self.staged, &command, slot, &key);
        self.notice = stolen.map(|c| Ok(format!("{} was taken from “{}”.", key_label(&key), self.describe(&c))));
    }

    fn mouse_pressed(&mut self, button: MouseButton, cx: &mut Context<Self>) {
        let n = match button {
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 3,
            MouseButton::Navigate(NavigationDirection::Back) => 4,
            MouseButton::Navigate(NavigationDirection::Forward) => 5,
        };
        self.assign(controls::mouse_key(n));
        cx.notify();
    }

    fn wheel(&mut self, e: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let dy = match e.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(p) => p.y,
        };
        if dy != 0. {
            self.assign((if dy > 0. { "mwheelup" } else { "mwheeldown" }).into());
            cx.notify();
        }
    }

    /// Shift, Ctrl, Alt and Caps Lock only arrive as modifier changes.
    fn modifiers_changed(&mut self, e: &ModifiersChangedEvent, cx: &mut Context<Self>) {
        if self.capture.is_none() {
            return;
        }
        let m = e.modifiers;
        let key = if m.shift {
            "shift"
        } else if m.control {
            "ctrl"
        } else if m.alt {
            "alt"
        } else if e.capslock.on != self.capslock {
            "capslock"
        } else {
            return;
        };
        self.assign(key.into());
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.running(cx) {
            self.notice = Some(Err("Quit the game first: it rewrites config.cfg when it quits.".into()));
            return;
        }
        let had_file = self.path.exists();
        self.notice = Some(match self.staged.save() {
            Ok(()) => {
                self.base = self.staged.clone();
                self.fresh = false;
                self.mtime = self.modified();
                Ok(if had_file {
                    "Saved config.cfg (the previous version is config.cfg.bak).".into()
                } else {
                    "Created config.cfg.".into()
                })
            }
            Err(e) => Err(format!("{e:#}")),
        });
    }

    fn slot_button(&self, command: &str, slot: usize, key: Option<&String>, cx: &mut Context<Self>) -> Button {
        let capturing = self.capture.as_ref().is_some_and(|(c, s)| c == command && *s == slot);
        let id = SharedString::from(format!("slot-{slot}-{command}"));
        let button = Button::new(id).small().w(px(130.));
        let button = match (capturing, key) {
            (true, _) => button.primary().label("Press a key…"),
            (false, Some(k)) => button.outline().label(key_label(k)),
            (false, None) => button.ghost().label("—"),
        };
        let command = command.to_owned();
        button.on_click(cx.listener(move |this, _, window, cx| this.start_capture(command.clone(), slot, window, cx)))
    }

    fn render_section(&self, s: &Section, cx: &mut Context<Self>) -> Div {
        let mut body = section(&title_case(&s.title), "", cx);
        for a in &s.actions {
            let keys = self.staged.keys_for(&a.command);
            let default: Vec<String> =
                self.defaults.iter().filter(|(_, c)| c.eq_ignore_ascii_case(&a.command)).map(|(k, _)| k.clone()).collect();
            let command = a.command.clone();
            body = body.child(
                h_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .child(div().text_sm().overflow_hidden().text_ellipsis().whitespace_nowrap().child(capitalize(&a.description)))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(cx.theme().muted_foreground)
                                    .whitespace_nowrap()
                                    .child(a.command.clone()),
                            ),
                    )
                    .children((0..SLOTS).map(|i| self.slot_button(&a.command, i, keys.get(i), cx)))
                    .child(div().w(px(28.)).flex_none().when(keys != default, |d| {
                        d.child(
                            Button::new(SharedString::from(format!("reset-{command}")))
                                .ghost()
                                .xsmall()
                                .icon(Lucide::RotateCcw)
                                .tooltip("Reset to default")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let stolen = controls::set_keys(&mut this.staged, &command, &default);
                                    let taken: Vec<String> = stolen
                                        .iter()
                                        .map(|(k, c)| format!("{} was taken from “{}”", key_label(k), this.describe(c)))
                                        .collect();
                                    this.notice = (!taken.is_empty()).then(|| Ok(format!("{}.", taken.join("; "))));
                                    cx.notify();
                                })),
                        )
                    })),
            );
        }
        body
    }

    fn render_other(&self, cx: &mut Context<Self>) -> Option<Div> {
        let known = |c: &str| self.sections.iter().flat_map(|s| &s.actions).any(|a| a.command.eq_ignore_ascii_case(c));
        let other: Vec<_> = self.staged.binds().into_iter().filter(|(_, c)| !known(c)).collect();
        if other.is_empty() {
            return None;
        }
        let mut body = section("Other bindings", "Keys bound to commands that aren't in the game's key list.", cx);
        for (key, command) in other {
            let removable = key != "escape";
            let k = key.clone();
            body = body.child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(130.)).flex_none().text_sm().child(key_label(&key)))
                    .child(div().flex_1().min_w_0().text_sm().font_family("monospace").overflow_hidden().text_ellipsis().child(command))
                    .child(div().w(px(28.)).flex_none().when(removable, |d| {
                        d.child(
                            Button::new(SharedString::from(format!("unbind-{key}")))
                                .ghost()
                                .xsmall()
                                .icon(Lucide::Trash)
                                .tooltip("Remove this binding")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.staged.unbind(&k);
                                    this.notice = None;
                                    cx.notify();
                                })),
                        )
                    })),
            );
        }
        Some(body)
    }
}

impl Render for ControlsTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Without staged changes, show what is on disk now (the game rewrites config.cfg on quit).
        if self.capture.is_none() && self.staged.text() == self.base.text() && self.modified() != self.mtime {
            self.load();
        }
        let running = self.running(cx);
        let dirty = self.dirty();
        let capture = self.capture.clone();
        let toolbar = h_flex()
            .gap_2()
            .child(Button::new("save").primary().small().icon(Lucide::Save).label("Save").disabled(!dirty || running).on_click(
                cx.listener(|this, _, _, cx| {
                    this.save(cx);
                    cx.notify();
                }),
            ))
            .child(
                Button::new("revert")
                    .outline()
                    .small()
                    .icon(Lucide::Undo2)
                    .label("Revert")
                    .disabled(self.staged.text() == self.base.text())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.load();
                        this.notice = None;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(if dirty { "Unsaved changes".to_owned() } else { self.path.display().to_string() }),
            )
            .child(Button::new("reset-all").ghost().small().icon(Lucide::RotateCcw).label("Reset to defaults").on_click(cx.listener(
                |this, _, _, cx| {
                    this.confirm_reset = true;
                    cx.notify();
                },
            )));

        let mut list = v_flex().id("controls-scroll").flex_1().min_h_0().overflow_y_scrollbar().px_6().pb_6().gap_4();
        for s in &self.sections {
            list = list.child(self.render_section(s, cx));
        }
        if self.sections.is_empty() {
            list = list
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child("No key list (gfx/shell/kb_act.lst) in ts or valve."));
        }
        if let Some(other) = self.render_other(cx) {
            list = list.child(other);
        }

        v_flex()
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, _, cx| this.modifiers_changed(e, cx)))
            .child(
                v_flex()
                    .px_6()
                    .pt_4()
                    .pb_3()
                    .gap_2()
                    .child(toolbar)
                    .when(running, |d| {
                        d.child(Alert::warning(
                            "controls-running",
                            "The game is running. It rewrites config.cfg when it quits, so Save is off until then.",
                        ))
                    })
                    .when(self.fresh, |d| {
                        d.child(Alert::info("controls-fresh", "There is no config.cfg yet. These are the default keys; Save creates it."))
                    })
                    .when(self.confirm_reset, |d| {
                        d.child(
                            h_flex()
                                .gap_2()
                                .p_3()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().warning)
                                .child(div().flex_1().text_sm().child("Replace every key binding with the defaults?"))
                                .child(Button::new("reset-yes").danger().small().label("Reset").on_click(cx.listener(|this, _, _, cx| {
                                    controls::reset_all(&mut this.staged, &this.defaults);
                                    this.confirm_reset = false;
                                    this.notice = Some(Ok("Reset to the default keys. Save to keep them.".into()));
                                    cx.notify();
                                })))
                                .child(Button::new("reset-no").ghost().small().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                                    this.confirm_reset = false;
                                    cx.notify();
                                }))),
                        )
                    })
                    // One fixed-height line, so the list doesn't move while keys are captured.
                    .child({
                        let (text, color) = match (&capture, &self.notice) {
                            (Some((command, _)), _) => (
                                format!(
                                    "Press a key, mouse button or wheel for “{}”. Esc cancels, Backspace or Delete clears.",
                                    capitalize(&self.describe(command))
                                ),
                                cx.theme().info,
                            ),
                            (None, Some(Ok(m))) => (m.clone(), cx.theme().success),
                            (None, Some(Err(e))) => (e.clone(), cx.theme().danger),
                            (None, None) => (String::new(), cx.theme().muted_foreground),
                        };
                        div().h(px(20.)).text_sm().text_color(color).overflow_hidden().text_ellipsis().whitespace_nowrap().child(text)
                    }),
            )
            .child(list)
            // While capturing, a transparent layer takes every click and wheel turn.
            .when(capture.is_some(), |d| {
                d.child(
                    div()
                        .id("capture-layer")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .on_any_mouse_down(cx.listener(|this, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.mouse_pressed(e.button, cx);
                        }))
                        .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| {
                            cx.stop_propagation();
                            this.wheel(e, cx);
                        })),
                )
            })
    }
}

/// `PLAYER MOVEMENT` as `Player movement`.
fn title_case(s: &str) -> String {
    if s.chars().any(|c| c.is_lowercase()) { s.to_owned() } else { capitalize(&s.to_lowercase()) }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}
