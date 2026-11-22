#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
extern crate gpui_kit as gpui;
mod analysis;
mod artwork;
mod assets;
mod audio;
mod browser;
mod cache;
mod catalog;
mod collection;
mod cue;
mod diagnostics;
mod discovery;
mod folders;
#[path = "levelMeter.rs"]
mod level_meter;
mod levels;
mod library;
mod location;
mod movie;
mod overview;
mod panels;
mod places;
mod player;
mod profile;
mod queue;
mod recording;
mod release;
mod signal;
mod store;
mod system;
mod table;
mod theme;
mod timeline;
mod waveform;
mod workspace;

use gpui_kit::{component::TitleBar, prelude::*, *};
use workspace::*;
fn main() {
    diagnostics::start();
    let arg = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .and_then(|p| std::path::absolute(p).ok());
    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::install(theme::Appearance::Dark, cx);
            workspace::bind_keys(cx);
            location::bind_keys(cx);
            panels::queue::bind_keys(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1240.), px(780.)),
                    cx,
                ))),
                window_min_size: Some(size(px(720.), px(460.))),
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
