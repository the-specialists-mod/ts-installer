//! The controls tab.

use gpui_kit::component::{ActiveTheme, v_flex};
use gpui_kit::*;

use super::AppState;

pub struct ControlsTab {
    state: Entity<AppState>,
    /// The installation this tab shows.
    id: String,
}

impl ControlsTab {
    pub fn new(state: Entity<AppState>, id: String, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { state, id }
    }
}

impl Render for ControlsTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = (&self.state, &self.id);
        v_flex().size_full().p_6().text_color(cx.theme().muted_foreground).child("Controls: coming soon")
    }
}
