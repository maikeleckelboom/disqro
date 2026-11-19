#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
extern crate gpui_kit as gpui;
mod artwork;
mod assets;
mod audio;
mod browser;
mod cue;
mod library;
mod movie;
mod overview;
mod player;
mod profile;
mod recording;
mod table;
mod workspace;

use gpui_kit::{
    component::{Theme, ThemeMode, TitleBar},
    prelude::*,
    *,
};
use workspace::*;
fn main() {
    let arg = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .and_then(|p| std::path::absolute(p).ok());
    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            Theme::update(cx, |t| {
                t.font_family = "Segoe UI".into();
                t.font_size = px(15.);
                t.colors.primary = rgb(0x77d7cb).into();
                t.colors.primary_foreground = rgb(0x112b2b).into();
                t.colors.ring = t.colors.primary;
                t.colors.background = rgb(0x171c20).into();
                t.colors.table = t.colors.background;
                t.colors.table_head_foreground = rgb(0xabbbb9).into();
                t.colors.sidebar = rgb(0x151a1e).into();
                t.colors.tab_bar = rgb(0x1c2227).into();
                t.colors.title_bar = rgb(0x151a1e).into();
                t.colors.border = rgb(0x323a40).into();
            });
            cx.bind_keys([
                KeyBinding::new("ctrl-o", OpenFolder, Some("Disqro")),
                KeyBinding::new("ctrl-f", FindTrack, Some("Disqro")),
                KeyBinding::new("space", PlayPause, Some("Disqro && !Input")),
                KeyBinding::new("enter", PlaySelected, Some("DataTable")),
                KeyBinding::new("enter", OpenDirectory, Some("Tree")),
                KeyBinding::new("alt-left", GoBack, Some("Disqro")),
            ]);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1130.), px(715.)),
                    cx,
                ))),
                window_min_size: Some(size(px(640.), px(440.))),
                app_id: Some("Disqro.Player".into()),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, move |window, cx| {
                window.set_window_title("Disqro");
                cx.new(|cx| Workspace::new(arg, window, cx))
            })
            .expect("Could not open Disqro");
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit()
                }
            })
            .detach();
            cx.activate(true);
        });
}
