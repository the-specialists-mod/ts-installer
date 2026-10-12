//! The banner that offers the launcher's own update: shown under the tab bar of an installation,
//! or under the header on the other screens.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use ts_launcher::progress::{Event, Progress};
use ts_launcher::update::{self, Update};

use super::theme;

pub struct UpdateBanner {
    state: UpdateState,
}

/// The launcher's own update.
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

impl UpdateBanner {
    /// Checks for an update in the background, unless that is turned off.
    pub fn new(cx: &mut Context<Self>) -> Self {
        if !update::disabled() {
            cx.spawn(async move |this, cx| {
                let found = cx.background_executor().spawn(async { update::check() }).await;
                match found {
                    Ok(Some(u)) => {
                        let _ = this.update(cx, |this, cx| {
                            this.state = UpdateState::Available(u);
                            cx.notify();
                        });
                    }
                    Ok(None) => {}
                    Err(e) => log::warn!("checking for a launcher update: {e:#}"),
                }
            })
            .detach();
        }
        Self { state: UpdateState::Hidden }
    }

    fn install_update(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Available(u) = &self.state else { return };
        let u = u.clone();
        self.state = UpdateState::Installing { tag: u.tag.clone(), bytes: (0, None) };
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
                    this.state = match (msg, &this.state) {
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
        let UpdateState::Installed { exe, .. } = &self.state else { return };
        match std::process::Command::new(exe).spawn() {
            Ok(_) => cx.quit(),
            Err(e) => self.state = UpdateState::Failed(format!("start {}: {e}", exe.display())),
        }
        cx.notify();
    }

    fn render_banner(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let later = |cx: &mut Context<Self>| {
            Button::new("update-later").ghost().small().label("Later").on_click(cx.listener(|this, _, _, cx| {
                this.state = UpdateState::Hidden;
                cx.notify();
            }))
        };
        let row = h_flex().px_6().py_2().gap_3().bg(theme::panel()).border_b_1().border_color(cx.theme().border).text_sm();
        Some(match &self.state {
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
}

impl Render for UpdateBanner {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_banner(cx).unwrap_or_else(|| div().into_any_element())
    }
}
