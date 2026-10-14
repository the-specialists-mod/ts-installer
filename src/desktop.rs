//! The app icon, and on Linux a desktop entry so Wayland compositors show it: they take a
//! window's icon from the `.desktop` file named after its app id, not from the window.

use std::sync::Arc;

/// The app id the window is opened with; the desktop entry and icon are named after it.
pub const APP_ID: &str = "ts-launcher";

const ICON_PNG: &[u8] = include_bytes!("../assets/icon/icon.png");

/// The icon as pixels, for X11 (which takes it from the window). 128px, because the property goes
/// in one X request, and a 512px icon is larger than servers without BIG-REQUESTS accept.
pub fn icon() -> Option<Arc<image::RgbaImage>> {
    let icon = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png).ok()?;
    Some(Arc::new(icon.resize(128, 128, image::imageops::FilterType::Lanczos3).to_rgba8()))
}

/// Writes the icon and a desktop entry starting this executable into the user's data directory,
/// so the launcher shows its icon and appears in the application menu. Files already up to date
/// are left alone.
#[cfg(target_os = "linux")]
pub fn register() -> anyhow::Result<()> {
    use anyhow::Context;
    let data = dirs::data_dir().context("find the data directory")?;
    let exe = std::env::current_exe().context("find the running launcher")?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=The Specialists Launcher\nComment=Install and play The Specialists 3.0\n\
         Exec=\"{}\"\nIcon={APP_ID}\nStartupWMClass={APP_ID}\nCategories=Game;\nTerminal=false\n",
        exe.display()
    );
    write_if_changed(&data.join("icons/hicolor/512x512/apps").join(format!("{APP_ID}.png")), ICON_PNG)?;
    write_if_changed(&data.join("applications").join(format!("{APP_ID}.desktop")), entry.as_bytes())
}

#[cfg(target_os = "linux")]
fn write_if_changed(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    use anyhow::Context;
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_decodes() {
        let icon = super::icon().expect("decode the icon");
        assert_eq!(icon.dimensions(), (128, 128));
    }
}
