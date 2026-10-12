//! The download cache screen: what is cached, how big it is, and deleting it.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use ts_launcher::cache::{self, CacheEntry};

use super::{human_bytes, theme};

pub enum CacheEvent {
    Back,
}

pub struct CacheView {
    entries: Vec<CacheEntry>,
    /// "Clear all" was clicked once: ask before doing it.
    confirm_clear: bool,
    status: Option<Result<String, String>>,
}

impl EventEmitter<CacheEvent> for CacheView {}

impl CacheView {
    pub fn new() -> Self {
        Self { entries: cache::entries(), confirm_clear: false, status: None }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.entries = cache::entries();
        cx.notify();
    }
}

fn when(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    super::human_time(secs)
}

impl Render for CacheView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total: u64 = self.entries.iter().map(|e| e.size).sum();
        let muted = cx.theme().muted_foreground;
        v_flex()
            .id("cache-scroll")
            .size_full()
            .overflow_y_scrollbar()
            .p_8()
            .gap_4()
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        Button::new("cache-back")
                            .ghost()
                            .icon(IconName::ArrowLeft)
                            .tooltip("Back")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CacheEvent::Back))),
                    )
                    .child(theme::heading("Download cache", 30.)),
            )
            .child(div().text_sm().text_color(muted).child(format!(
                "Installs, repairs, resets and validation use these files instead of downloading them again; they are \
                 checked against the release first and downloaded again when the release changed. When GitHub can't be \
                 reached, the cached files are used as they are. {} in {}.",
                human_bytes(total),
                cache::dir().display()
            )))
            .child(
                h_flex()
                    .gap_3()
                    .child(Button::new("cache-open").outline().small().icon(IconName::FolderOpen).label("Open folder").on_click(
                        |_, _, cx| {
                            let dir = cache::dir();
                            let _ = std::fs::create_dir_all(&dir);
                            cx.open_with_system(&dir);
                        },
                    ))
                    .child(Button::new("cache-refresh").outline().small().icon(Lucide::RefreshCw).label("Refresh").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.status = None;
                            this.reload(cx);
                        },
                    )))
                    .when(!self.entries.is_empty(), |r| {
                        r.child(Button::new("cache-clear").ghost().small().icon(IconName::Delete).label("Clear all…").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.confirm_clear = true;
                                cx.notify();
                            }),
                        ))
                    }),
            )
            .when(self.confirm_clear, |d| {
                d.child(
                    h_flex()
                        .gap_3()
                        .child(div().flex_1().text_sm().text_color(cx.theme().warning).child(format!(
                            "Delete all {} of cached downloads? The next install or repair downloads what it needs again.",
                            human_bytes(total)
                        )))
                        .child(Button::new("clear-cancel").ghost().small().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_clear = false;
                            cx.notify();
                        })))
                        .child(Button::new("clear-confirm").danger().small().label("Delete all").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.confirm_clear = false;
                                this.status = Some(match cache::clear() {
                                    Ok(()) => Ok(format!("Deleted {} of cached downloads", human_bytes(total))),
                                    Err(e) => Err(format!("{e:#}")),
                                });
                                this.reload(cx);
                            },
                        ))),
                )
            })
            .when_some(self.status.clone(), |d, s| {
                d.child(match s {
                    Ok(m) => Alert::success("cache-status", m).into_any_element(),
                    Err(e) => Alert::error("cache-status", e).into_any_element(),
                })
            })
            .when(self.entries.is_empty(), |d| d.child(div().text_sm().text_color(muted).child("Nothing is cached.")))
            .children(self.entries.iter().enumerate().map(|(ix, e)| {
                let path = e.path.clone();
                let name = e.name.clone();
                h_flex()
                    .gap_4()
                    .px_4()
                    .py_3()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(v_flex().flex_1().min_w_0().gap_1().child(div().text_sm().child(e.what.clone())).child(
                        div().text_xs().text_color(muted).overflow_hidden().text_ellipsis().whitespace_nowrap().child(format!(
                            "{} · {}",
                            e.name,
                            e.modified.map(when).unwrap_or_default()
                        )),
                    ))
                    .child(div().flex_none().text_sm().child(human_bytes(e.size)))
                    .child(Button::new(("cache-delete", ix)).ghost().small().icon(IconName::Delete).tooltip("Delete").on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.status = Some(match cache::remove(&path) {
                                Ok(()) => Ok(format!("Deleted {name}")),
                                Err(e) => Err(format!("{e:#}")),
                            });
                            this.reload(cx);
                        }),
                    ))
            }))
    }
}
