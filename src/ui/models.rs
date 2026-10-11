//! The player models tab: the models in `ts/models/player` with their previews, a turnable 3D
//! view of the selected one, and choosing the model the game uses (the `model` cvar).

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use ts_launcher::mdl::{self, Model};
use ts_launcher::models::{self, Import, PlayerModel, Rgba};

use super::{AppState, detail_row, human_bytes};

/// Size of the 3D view. Small enough to re-render while dragging in a debug build.
const VIEW_W: usize = 320;
const VIEW_H: usize = 400;
/// Radians per pixel dragged.
const TURN_SPEED: f32 = 0.012;

struct Card {
    model: PlayerModel,
    preview: Option<Arc<RenderImage>>,
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    ThreeD,
    Picture,
}

/// The 3D view of the selected model: the parsed model and the latest rendered frame.
#[derive(Default)]
struct Turntable {
    /// Which model folder this is for.
    dir: PathBuf,
    model: Option<Arc<Model>>,
    error: Option<String>,
    image: Option<Arc<RenderImage>>,
    yaw: f32,
    /// A render is running in the background.
    busy: bool,
    /// The next frame to draw once the running one is done: (yaw, supersampling).
    wanted: Option<(f32, usize)>,
    /// Mouse x and yaw where the current drag started.
    drag: Option<(f32, f32)>,
}

pub struct ModelsTab {
    state: Entity<AppState>,
    /// The installation this tab shows.
    id: String,
    ts: PathBuf,
    cards: Option<Vec<Card>>,
    selected: Option<String>,
    /// The `model` cvar in config.cfg.
    current: Option<String>,
    status: Option<Result<String, String>>,
    /// An import waiting for "Replace" because the model folder exists.
    confirm: Option<Import>,
    view: View,
    turntable: Turntable,
    /// Images no longer shown, to free from the sprite atlas on the next render.
    stale: Vec<Arc<RenderImage>>,
}

impl ModelsTab {
    pub fn new(state: Entity<AppState>, id: String, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ts = state.read(cx).instance(&id).map(|i| i.ts_dir()).unwrap_or_default();
        let mut this = Self {
            state,
            id,
            ts,
            cards: None,
            selected: None,
            current: None,
            status: None,
            confirm: None,
            view: View::ThreeD,
            turntable: Turntable::default(),
            stale: Vec::new(),
        };
        this.reload(None, cx);
        this
    }

    /// Reads the model folders and their previews in the background, then selects `select` (or
    /// keeps the selection, or picks the model in use).
    fn reload(&mut self, select: Option<String>, cx: &mut Context<Self>) {
        let ts = self.ts.clone();
        self.current = models::current(&ts);
        cx.spawn(async move |this, cx| {
            let cards = cx
                .background_executor()
                .spawn(async move {
                    models::list(&ts)
                        .into_iter()
                        .map(|model| {
                            let preview = model
                                .bmp
                                .as_ref()
                                .and_then(|p| std::fs::read(p).ok())
                                .and_then(|b| models::decode_bmp(&b).ok())
                                .map(|img| render_image(&img));
                            Card { model, preview }
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                for old in this.cards.take().into_iter().flatten() {
                    this.stale.extend(old.preview);
                }
                let has = |n: &str| cards.iter().any(|c| c.model.name.eq_ignore_ascii_case(n));
                let pick = select
                    .or_else(|| this.selected.clone())
                    .filter(|n| has(n))
                    .or_else(|| this.current.clone().filter(|n| has(n)))
                    .or_else(|| cards.first().map(|c| c.model.name.clone()));
                this.cards = Some(cards);
                this.selected = None;
                if let Some(name) = pick {
                    this.select(name, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn selected_card(&self) -> Option<&Card> {
        let name = self.selected.as_deref()?;
        self.cards.as_ref()?.iter().find(|c| c.model.name == name)
    }

    fn is_current(&self, name: &str) -> bool {
        self.current.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(name))
    }

    fn select(&mut self, name: String, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&name) {
            return;
        }
        self.selected = Some(name);
        self.load_turntable(cx);
        cx.notify();
    }

    /// Parses the selected model for the 3D view (once per model) and draws it.
    fn load_turntable(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.selected_card() else { return };
        let (dir, mdl_path) = (card.model.dir.clone(), card.model.mdl.clone());
        if self.view != View::ThreeD || self.turntable.dir == dir {
            return;
        }
        let old = std::mem::replace(&mut self.turntable, Turntable { dir: dir.clone(), yaw: 0.0, ..Default::default() });
        self.stale.extend(old.image);
        let Some(path) = mdl_path else {
            self.turntable.error = Some("This folder has no .mdl file named after it, so the game can't use it.".into());
            return;
        };
        cx.spawn(async move |this, cx| {
            let model = cx.background_executor().spawn(async move { Model::load(&path) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.turntable.dir != dir {
                    return;
                }
                match model {
                    Ok(m) => {
                        this.turntable.model = Some(Arc::new(m));
                        this.draw(0.0, 2, cx);
                    }
                    Err(e) => this.turntable.error = Some(format!("Can't show this model: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks for a frame at `yaw`. While one renders, only the latest request is kept.
    fn draw(&mut self, yaw: f32, ssaa: usize, cx: &mut Context<Self>) {
        self.turntable.yaw = yaw;
        self.turntable.wanted = Some((yaw, ssaa));
        if self.turntable.busy {
            return;
        }
        let (Some(model), Some((yaw, ssaa))) = (self.turntable.model.clone(), self.turntable.wanted.take()) else { return };
        self.turntable.busy = true;
        let dir = self.turntable.dir.clone();
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_executor()
                .spawn(async move {
                    let pixels = mdl::render(&model, yaw, VIEW_W, VIEW_H, ssaa);
                    render_image(&Rgba { width: VIEW_W, height: VIEW_H, pixels })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.turntable.dir != dir {
                    this.stale.push(image);
                    return;
                }
                this.turntable.busy = false;
                if let Some(old) = this.turntable.image.replace(image) {
                    this.stale.push(old);
                }
                if let Some((yaw, ssaa)) = this.turntable.wanted {
                    this.draw(yaw, ssaa, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn use_model(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.selected.clone() else { return };
        if self.state.update(cx, |s, _| s.game_running()) {
            self.status = Some(Err("The game is running. Quit it first: it rewrites config.cfg when it exits.".into()));
        } else {
            self.status = Some(match models::select(&self.ts, &name) {
                Ok(()) => {
                    self.current = Some(name.clone());
                    Ok(format!("You'll play as {name} (model \"{name}\" in ts/config.cfg)."))
                }
                Err(e) => Err(format!("Could not save the model: {e:#}")),
            });
        }
        cx.notify();
    }

    fn add_model(&mut self, cx: &mut Context<Self>) {
        self.status = None;
        self.confirm = None;
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add this model".into()),
        });
        cx.spawn(async move |this, cx| {
            let path = match rx.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                Ok(Err(e)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.status = Some(Err(format!("Could not open a folder picker: {e:#}")));
                        cx.notify();
                    });
                    None
                }
                _ => None,
            };
            let Some(path) = path else { return };
            let _ = this.update(cx, |this, cx| {
                match models::plan_import(&path, &this.ts) {
                    Ok(plan) if plan.exists => this.confirm = Some(plan),
                    Ok(plan) => this.finish_import(plan, false, cx),
                    Err(e) => this.status = Some(Err(format!("{e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn finish_import(&mut self, plan: Import, overwrite: bool, cx: &mut Context<Self>) {
        self.confirm = None;
        match models::import(&plan, &self.ts, overwrite) {
            Ok(dest) => {
                let name = dest.file_name().unwrap_or_default().to_string_lossy().into_owned();
                self.status = Some(Ok(format!("Added {name} ({} files).", plan.files.len())));
                // Re-read it even if it was selected before.
                self.turntable.dir = PathBuf::new();
                self.selected = None;
                self.reload(Some(name), cx);
            }
            Err(e) => self.status = Some(Err(format!("Could not add the model: {e:#}"))),
        }
        cx.notify();
    }

    fn render_card(&self, ix: usize, card: &Card, cx: &mut Context<Self>) -> impl IntoElement {
        let m = &card.model;
        let selected = self.selected.as_deref() == Some(m.name.as_str());
        let current = self.is_current(&m.name);
        let name = m.name.clone();
        v_flex()
            .id(("model-card", ix))
            .w(px(126.))
            .p_2()
            .gap_1()
            .rounded(cx.theme().radius)
            .border_2()
            .border_color(if selected { cx.theme().primary } else { cx.theme().border })
            .when(selected, |d| d.bg(cx.theme().accent))
            .cursor_pointer()
            .hover(|s| s.bg(cx.theme().accent))
            .on_click(cx.listener(move |this, _, _, cx| this.select(name.clone(), cx)))
            .child(preview_box(card.preview.clone(), 106., 129., cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(div().flex_1().min_w_0().font_semibold().text_sm().overflow_hidden().text_ellipsis().child(m.name.clone()))
                    .when(current, |d| d.child(Icon::new(IconName::Check).small().text_color(cx.theme().primary))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(m.author.as_deref().map(author_name).unwrap_or_else(|| "unknown author".into())),
            )
    }

    fn render_detail(&self, card: &Card, cx: &mut Context<Self>) -> impl IntoElement {
        let m = &card.model;
        let current = self.is_current(&m.name);
        let t = &self.turntable;
        let viewer = match self.view {
            View::Picture => preview_box(card.preview.clone(), 246., 300., cx).into_any_element(),
            View::ThreeD => {
                let body = if let Some(e) = &t.error {
                    div().p_4().text_sm().text_color(cx.theme().muted_foreground).child(e.clone()).into_any_element()
                } else if let Some(image) = &t.image {
                    img(ImageSource::Render(image.clone())).w(px(VIEW_W as f32)).h(px(VIEW_H as f32)).into_any_element()
                } else {
                    Spinner::new().large().into_any_element()
                };
                div()
                    .id("turntable")
                    .w(px(VIEW_W as f32))
                    .h(px(VIEW_H as f32))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .cursor(if t.drag.is_some() { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &MouseDownEvent, _, cx| {
                            this.turntable.drag = Some((f32::from(e.position.x), this.turntable.yaw));
                            cx.notify();
                        }),
                    )
                    .child(body)
                    .into_any_element()
            }
        };
        let seq = t
            .model
            .as_ref()
            .map(|m| format!("{} triangles, {} bones · pose: {}", m.triangles.len(), m.bones, m.sequences[m.sequence].label));
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().text_lg().font_semibold().child(m.title.clone().unwrap_or_else(|| m.name.clone())))
                    .when(current, |d| d.child(Tag::primary().small().child("In use"))),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(Button::new("view-3d").small().label("3D").selected(self.view == View::ThreeD).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.view = View::ThreeD;
                            this.load_turntable(cx);
                            cx.notify();
                        },
                    )))
                    .child(Button::new("view-picture").small().label("Picture").selected(self.view == View::Picture).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.view = View::Picture;
                            cx.notify();
                        },
                    )))
                    .child(div().flex_1())
                    .when(self.view == View::ThreeD && t.model.is_some(), |d| {
                        d.child(div().text_xs().text_color(cx.theme().muted_foreground).child("Drag to turn"))
                    }),
            )
            .child(h_flex().justify_center().child(viewer))
            .when(self.view == View::ThreeD, |d| {
                d.when_some(seq, |d, s| d.child(div().text_xs().text_color(cx.theme().muted_foreground).child(s)))
            })
            .child(
                Button::new("use-model")
                    .primary()
                    .icon(IconName::Check)
                    .label(if current { "This is your model".to_owned() } else { format!("Use {}", m.name) })
                    .disabled(current || m.mdl.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.use_model(cx))),
            )
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child(
                "Sets the model cvar in ts/config.cfg. Team play servers (team deathmatch, The One) give you a model from your \
                 team's list instead (mp_teammodels or the map's team models).",
            ))
            .child(detail_row("Model name", m.name.clone(), cx))
            .when_some(m.author.clone(), |d, a| d.child(detail_row("Author", a, cx)))
            .child(detail_row("Size", human_bytes(m.size()), cx))
            .children(m.files.iter().map(|(f, n)| detail_row(&format!("  {f}"), human_bytes(*n), cx)))
            .when(!m.text.is_empty(), |d| {
                d.child(
                    div()
                        .p_3()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().secondary)
                        .text_xs()
                        .font_family("monospace")
                        .child(m.text.trim_end().to_owned()),
                )
            })
    }
}

impl Render for ModelsTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.stale.drain(..) {
            let _ = window.drop_image(image);
        }
        let _ = &self.id;
        let dir = models::player_dir(&self.ts);
        let count = self.cards.as_ref().map(|c| c.len());
        let missing_current = self
            .current
            .clone()
            .filter(|c| self.cards.as_ref().is_some_and(|cards| !cards.iter().any(|k| k.model.name.eq_ignore_ascii_case(c))));

        let grid = match &self.cards {
            None => h_flex().p_6().justify_center().child(Spinner::new()).into_any_element(),
            Some(cards) if cards.is_empty() => div()
                .p_6()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(format!("No player models in {}.", dir.display()))
                .into_any_element(),
            Some(cards) => {
                let cards: Vec<_> = cards.iter().enumerate().map(|(ix, c)| self.render_card(ix, c, cx).into_any_element()).collect();
                h_flex().flex_wrap().items_start().gap_3().children(cards).into_any_element()
            }
        };

        let left = v_flex()
            .flex_1()
            .min_w_0()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().text_sm().text_color(cx.theme().muted_foreground).child(match count {
                        Some(n) => format!("{n} models"),
                        None => "Loading…".into(),
                    }))
                    .child(
                        Button::new("refresh")
                            .ghost()
                            .small()
                            .icon(Lucide::RefreshCw)
                            .tooltip("Reload the models folder")
                            .on_click(cx.listener(|this, _, _, cx| this.reload(None, cx))),
                    )
                    .child(Button::new("open-models").outline().small().icon(IconName::FolderOpen).label("Open folder").on_click(
                        cx.listener(|this, _, _, cx| {
                            let dir = models::player_dir(&this.ts);
                            let _ = std::fs::create_dir_all(&dir);
                            cx.open_with_system(&dir);
                        }),
                    ))
                    .child(
                        Button::new("add-model")
                            .outline()
                            .small()
                            .icon(IconName::Plus)
                            .label("Add model…")
                            .on_click(cx.listener(|this, _, _, cx| this.add_model(cx))),
                    ),
            )
            .when_some(self.confirm.clone(), |d, plan| {
                d.child(
                    v_flex()
                        .gap_2()
                        .child(Alert::warning("confirm-import", format!("models/player/{} already exists. Replace its files?", plan.name)))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("replace")
                                        .danger()
                                        .small()
                                        .label("Replace")
                                        .on_click(cx.listener(move |this, _, _, cx| this.finish_import(plan.clone(), true, cx))),
                                )
                                .child(Button::new("cancel-import").ghost().small().label("Cancel").on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.confirm = None;
                                        cx.notify();
                                    },
                                ))),
                        ),
                )
            })
            .when_some(self.status.clone(), |d, s| {
                d.child(match s {
                    Ok(m) => Alert::success("models-status", m).into_any_element(),
                    Err(e) => Alert::error("models-status", e).into_any_element(),
                })
            })
            .when_some(missing_current, |d, c| {
                d.child(Alert::warning("missing-current", format!("config.cfg selects \"{c}\", which isn't in models/player.")))
            })
            .child(grid);

        let detail = self.selected_card().map(|c| self.render_detail(c, cx).into_any_element());

        h_flex()
            .id("models-tab")
            .size_full()
            .items_start()
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                let Some((x0, yaw0)) = this.turntable.drag else { return };
                if e.pressed_button == Some(MouseButton::Left) {
                    this.draw(yaw0 + (f32::from(e.position.x) - x0) * TURN_SPEED, 1, cx);
                } else {
                    // Released outside the window: finish the drag.
                    this.turntable.drag = None;
                    this.draw(this.turntable.yaw, 2, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.turntable.drag.take().is_some() {
                        this.draw(this.turntable.yaw, 2, cx);
                        cx.notify();
                    }
                }),
            )
            .child(v_flex().id("models-grid").flex_1().min_w_0().h_full().overflow_y_scrollbar().p_6().child(left))
            .child(
                v_flex()
                    .id("model-detail")
                    .w(px(372.))
                    .flex_none()
                    .h_full()
                    .overflow_y_scrollbar()
                    .p_4()
                    .border_l_1()
                    .border_color(cx.theme().border)
                    .children(detail),
            )
    }
}

/// A preview picture, or a placeholder for models without one.
fn preview_box(image: Option<Arc<RenderImage>>, w: f32, h: f32, cx: &App) -> AnyElement {
    match image {
        Some(i) => img(ImageSource::Render(i)).w(px(w)).h(px(h)).rounded(cx.theme().radius).into_any_element(),
        None => div()
            .w(px(w))
            .h(px(h))
            .flex()
            .items_center()
            .justify_center()
            .rounded(cx.theme().radius)
            .bg(cx.theme().secondary)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child("No preview")
            .into_any_element(),
    }
}

/// "Morfeo <mail@example.com>" without the address.
fn author_name(a: &str) -> String {
    a.split('<').next().unwrap_or(a).trim().to_owned()
}

/// RGBA pixels as a GPUI image (which stores BGRA).
fn render_image(img: &Rgba) -> Arc<RenderImage> {
    let mut bgra = img.pixels.clone();
    for p in bgra.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(img.width as u32, img.height as u32, bgra).expect("buffer matches the size");
    Arc::new(RenderImage::new([image::Frame::new(buffer)]))
}
