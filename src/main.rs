#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ui;

use gpui_kit::*;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The command line prints its own progress; the UI logs it.
    let filter = if args.is_empty() { "ts_launcher=info,warn" } else { "warn" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(filter)).init();
    if !args.is_empty() {
        if let Err(e) = ts_launcher::cli::run(&args) {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(|cx| {
        gpui_kit::init(cx);
        ui::theme::init(cx);
        let bounds = Bounds::centered(None, size(px(880.), px(720.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("The Specialists Launcher".into()), ..Default::default() }),
            app_id: Some("ts-launcher".into()),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| ui::Launcher::new(window, cx))).expect("open the launcher window");
        cx.activate(true);
    });
}
