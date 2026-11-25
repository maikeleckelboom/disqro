// Modified by Longbridge for gpui-fast.
#![cfg(target_os = "windows")]

mod clipboard;
mod destination_list;
mod dialog;
mod direct_manipulation;
mod direct_write;
mod directx_atlas;
mod directx_devices;
mod directx_renderer;
mod dispatcher;
mod display;
mod events;
mod fast;
mod keyboard;
mod platform;
#[cfg(feature = "frame-statistics")]
pub mod frame_statistics;
mod system_notifications;
mod system_settings;
mod util;
mod vsync;
mod window;
mod wrapper;

pub(crate) use clipboard::*;
pub(crate) use destination_list::*;
pub(crate) use direct_write::*;
pub(crate) use directx_atlas::*;
pub(crate) use directx_devices::*;
pub(crate) use directx_renderer::FontInfo;
pub(crate) use directx_renderer::*;
pub(crate) use dispatcher::*;
pub(crate) use display::*;
pub(crate) use events::*;
pub(crate) use keyboard::*;
pub(crate) use platform::*;
pub(crate) use system_notifications::*;
pub(crate) use system_settings::*;
pub(crate) use util::*;
pub(crate) use vsync::*;
pub(crate) use window::*;
pub(crate) use wrapper::*;

pub use platform::WindowsPlatform;
pub use fast::dynamic_textures::{GpuTexture, with_d3d11};
pub use window::{CaptureHoverPause, pause_hover_during_capture, present_retained_next_frame};
#[cfg(feature = "frame-statistics")]
pub use window::qualify_device_recreation;

pub(crate) use windows::Win32::Foundation::HWND;
