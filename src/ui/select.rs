//! Choosing which installation to use when the launcher knows several.

use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use ts_launcher::install::{Runtime, ts_libs};

use super::{Launcher, Screen, human_time};

impl Launcher {
    pub(super) fn render_select(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = &self.state.read(cx).store;
        let last = store.last_used.clone();
        let cards: Vec<_> = store
            .instances
            .iter()
            .enumerate()
            .map(|(i, inst)| {
                let id = inst.id.clone();
                let missing = !inst.install.root.is_dir();
                let libs = ts_libs(&inst.ts_dir()).map(|l| l.label()).unwrap_or("TS missing");
                let icon = match inst.install.runtime {
                    Runtime::GoldSrc => IconName::Folder,
                    Runtime::Xash(_) => IconName::Globe,
                };
                let played = inst.last_played.map(human_time).unwrap_or_else(|| "never".into());
                h_flex()
                    .id(("installation", i))
                    .gap_4()
                    .p_4()
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(if last.as_deref() == Some(inst.id.as_str()) { cx.theme().primary } else { cx.theme().border })
                    .bg(cx.theme().secondary)
                    .cursor_pointer()
                    .hover(|s| s.border_color(cx.theme().primary))
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| {
                            this.notice = None;
                            this.show(Screen::Instance(id.clone()), cx)
                        }
                    }))
                    .child(div().text_color(cx.theme().primary).child(Icon::new(icon).large()))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(div().font_semibold().child(inst.name.clone()))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(inst.install.root.display().to_string()),
                            )
                            .child(
                                div().text_xs().text_color(if missing { cx.theme().danger } else { cx.theme().muted_foreground }).child(
                                    if missing {
                                        "Folder not found".to_owned()
                                    } else {
                                        format!("{} · {libs} · last played {played}", inst.install.runtime.label())
                                    },
                                ),
                            ),
                    )
                    .child(
                        Button::new(("forget", i))
                            .ghost()
                            .small()
                            .icon(IconName::Delete)
                            .tooltip("Remove from the launcher (files stay on disk)")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let saved = this.state.update(cx, |state, cx| {
                                    state.store.remove(&id);
                                    cx.notify();
                                    state.save()
                                });
                                this.notice = saved.err().map(Err);
                                if this.state.read(cx).store.instances.is_empty() {
                                    this.show(Screen::Welcome, cx);
                                }
                                cx.notify();
                            })),
                    )
            })
            .collect();
        v_flex()
            .id("select-scroll")
            .size_full()
            .overflow_y_scrollbar()
            .p_8()
            .gap_4()
            .child(div().text_2xl().font_bold().child("Choose an installation"))
            .when_some(self.notice.clone(), |d, n| {
                d.child(match n {
                    Ok(m) => Alert::success("select-notice", m).into_any_element(),
                    Err(e) => Alert::error("select-notice", e).into_any_element(),
                })
            })
            .children(cards)
            .child(
                h_flex().child(
                    Button::new("add-installation")
                        .outline()
                        .icon(IconName::Plus)
                        .label("Add installation…")
                        .on_click(cx.listener(|this, _, _, cx| this.show(Screen::Welcome, cx))),
                ),
            )
    }
}
