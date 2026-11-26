#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
extern crate gpui_kit as gpui;
mod analysis;
mod artwork;
mod assets;
mod audio;
mod browser;
mod cache;
mod capture;
mod catalog;
mod collection;
mod cue;
mod diagnostics;
mod disclosure;
mod discovery;
mod folders;
mod level_meter;
mod levels;
mod library;
mod listening;
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
mod spectral_history;
mod spectrum;
mod spectrum_worker;
mod stereo_field;
mod store;
mod system;
mod table;
mod theme;
mod timeline;
mod waveform_detail;
mod waveform_port_analysis;
#[cfg(feature = "qualification")]
mod waveform_port_qualification;
mod waveform_render;
mod waveform_source;
mod workspace;

use gpui_kit::{component::TitleBar, prelude::*, *};
use workspace::*;
fn main() {
    let mut args = std::env::args_os().skip(1);
    let mut source = args.next();
    #[cfg(feature = "qualification")]
    if source.as_deref() == Some(std::ffi::OsStr::new("--waveform-parity")) {
        std::process::exit(waveform_port_qualification::run(args));
    }
    if source.as_deref() == Some(std::ffi::OsStr::new("--qualify-silent")) {
        if !cfg!(feature = "qualification") || std::env::var_os("DISQRO_PROFILE_DIR").is_none() {
            std::process::exit(2);
        }
        audio::qualify_silently();
        source = args.next();
    }
    diagnostics::start();
    #[cfg(all(windows, feature = "qualification"))]
    {
        waveform_port_qualification::start_presentation_trace();
        waveform_port_qualification::arm_target_failure();
    }
    let arg = source
        .map(std::path::PathBuf::from)
        .and_then(|p| std::path::absolute(p).ok());
    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            diagnostics::observe_waveform_surfaces(cx);
            theme::install(theme::Appearance::Dark, cx);
            workspace::bind_keys(cx);
            disclosure::bind_keys(cx);
            location::bind_keys(cx);
            panels::queue::bind_keys(cx);
            panels::listening::bind_keys(cx);
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
                #[cfg(all(windows, feature = "qualification"))]
                waveform_port_qualification::exercise_device_recreation(window, cx);
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
