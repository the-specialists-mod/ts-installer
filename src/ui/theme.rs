//! The look of tsmod.org: grunge background, `#333` panels with black borders, red buttons, square
//! corners, Anton headings and the stencilled metal nav bar.

use std::borrow::Cow;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use gpui_kit::component::{Theme, ThemeConfig, ThemeMode};
use gpui_kit::*;

/// The site's display font, for headings and the nav bar.
pub const DISPLAY_FONT: &str = "Anton";

/// Panel background (the site's `.panel`).
pub fn panel() -> Hsla {
    rgb(0x333333).into()
}

/// Nav bar text: dark stencil, white when active.
pub fn stencil() -> Hsla {
    rgb(0x161616).into()
}

pub fn stencil_lit() -> Hsla {
    rgb(0xf4f4f4).into()
}

const THEME: &str = r##"{
    "name": "tsmod.org",
    "mode": "dark",
    "radius": 0,
    "radius.lg": 0,
    "shadow": false,
    "colors": {
        "background": "#2b2b2b",
        "foreground": "#ececec",
        "border": "#000000",
        "window.border": "#000000",
        "input.border": "#5a5a5a",
        "caret": "#edff6f",
        "ring": "#edff6f",
        "muted.background": "#2a2a2a",
        "muted.foreground": "#b4b4b4",
        "primary.background": "#a51d1d",
        "primary.hover.background": "#b92525",
        "primary.active.background": "#8a1717",
        "primary.foreground": "#ffffff",
        "secondary.background": "#4a4a4a",
        "secondary.hover.background": "#555555",
        "secondary.active.background": "#3d3d3d",
        "secondary.foreground": "#ececec",
        "accent.background": "#454545",
        "accent.foreground": "#ffffff",
        "popover.background": "#2b2b2b",
        "popover.foreground": "#ececec",
        "group_box.background": "#333333",
        "group_box.foreground": "#ececec",
        "danger.background": "#e05a4f",
        "danger.foreground": "#e05a4f",
        "link.foreground": "#b5bac8",
        "link.hover.foreground": "#edff6f",
        "link.active.foreground": "#edff6f",
        "selection.background": "#a51d1d",
        "drag_border": "#edff6f",
        "drop_target.background": "#edff6f19",
        "list.background": "#2b2b2b",
        "list.active.background": "#a51d1d40",
        "list.active.border": "#a51d1d",
        "list.even.background": "#ffffff06",
        "list.head.background": "#222222",
        "table.background": "#2b2b2b",
        "table.head.foreground": "#b4b4b4",
        "table.row.border": "#000000b3",
        "progress_bar.background": "#a51d1d",
        "slider.bar.background": "#a51d1d",
        "slider.thumb.background": "#ececec",
        "switch.background": "#4a4a4a",
        "scrollbar.background": "#00000000",
        "scrollbar.thumb.background": "#5a5a5ae6",
        "scrollbar.thumb.hover.background": "#6a6a6a",
        "skeleton.background": "#333333",
        "tab.background": "#00000000",
        "tab.active.background": "#333333",
        "tab.active.foreground": "#f4f4f4",
        "tab.foreground": "#b4b4b4",
        "tab_bar.background": "#00000000",
        "tab_bar.segmented.background": "#222222",
        "title_bar.background": "#000000",
        "title_bar.border": "#000000",
        "sidebar.background": "#2b2b2b",
        "sidebar.accent.background": "#454545",
        "sidebar.accent.foreground": "#ffffff",
        "sidebar.border": "#000000",
        "sidebar.foreground": "#ececec",
        "sidebar.primary.background": "#a51d1d",
        "sidebar.primary.foreground": "#ffffff",
        "description_list_label.background": "#2a2a2a",
        "description_list_label.foreground": "#b4b4b4",
        "accordion.background": "#2b2b2b",
        "chart.1": "#a51d1d",
        "chart.2": "#b5bac8",
        "chart.3": "#8a8a8a",
        "chart.4": "#666666",
        "chart.5": "#444444"
    }
}"##;

/// A heading in the display font, upper-cased like the site's.
pub fn heading(text: &str, size: f32) -> Div {
    div().font_family(DISPLAY_FONT).text_size(px(size)).line_height(px(size * 1.25)).child(text.to_uppercase())
}

/// Loads the fonts and applies the theme. Call after `gpui_kit::init`.
pub fn init(cx: &mut App) {
    let anton = include_bytes!("../../assets/Anton-Regular.ttf");
    if let Err(e) = cx.text_system().add_fonts(vec![Cow::Borrowed(anton)]) {
        log::warn!("loading the Anton font: {e:#}");
    }
    let config: ThemeConfig = serde_json::from_str(THEME).expect("the built-in theme parses");
    Theme::global_mut(cx).apply_config(&Rc::new(config));
    Theme::change(ThemeMode::Dark, None, cx);
    // The site asks for Verdana, which only Windows ships; elsewhere keep the system font.
    if cfg!(windows) {
        Theme::global_mut(cx).font_family = "Verdana".into();
    }
}

fn image(format: ImageFormat, bytes: &'static [u8]) -> Arc<Image> {
    Arc::new(Image::from_bytes(format, bytes.to_vec()))
}

/// The site's page background texture.
pub static GRUNGE: LazyLock<Arc<Image>> = LazyLock::new(|| image(ImageFormat::Jpeg, include_bytes!("../../assets/grunge.jpg")));
/// The brushed metal of the site's nav bar.
pub static METAL_BAR: LazyLock<Arc<Image>> = LazyLock::new(|| image(ImageFormat::Jpeg, include_bytes!("../../assets/metal-bar.jpg")));
/// "THE SPECIALISTS" on a dog tag.
pub static LOGO: LazyLock<Arc<Image>> = LazyLock::new(|| image(ImageFormat::Png, include_bytes!("../../assets/logo_ts.png")));
