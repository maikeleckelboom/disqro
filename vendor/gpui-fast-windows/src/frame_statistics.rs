//! Opt-in DXGI display statistics for native qualification.
use std::sync::OnceLock;
use windows::Win32::{
    Foundation::HWND,
    Graphics::Dxgi::{DXGI_FRAME_STATISTICS, IDXGISwapChain1},
    System::Performance::QueryPerformanceCounter,
    UI::WindowsAndMessaging::GetForegroundWindow,
};

pub struct FrameStatistics {
    pub qpc: i64,
    pub foreground: bool,
    pub queued: u32,
    pub present_status: i32,
    pub statistics_status: i32,
    pub display: DXGI_FRAME_STATISTICS,
}

static LISTENER: OnceLock<Box<dyn Fn(FrameStatistics) + Send + Sync>> = OnceLock::new();

pub fn listen(listener: impl Fn(FrameStatistics) + Send + Sync + 'static) {
    let _ = LISTENER.set(Box::new(listener));
}

pub(crate) fn record(chain: &IDXGISwapChain1, window: HWND, present_status: i32) {
    let Some(listener) = LISTENER.get() else { return };
    let mut qpc = 0;
    let mut display = DXGI_FRAME_STATISTICS::default();
    // SAFETY: the caller owns this window's live swap chain and the output
    // slots are valid for the lifetime of the system calls.
    unsafe {
        let _ = QueryPerformanceCounter(&mut qpc);
        let foreground = GetForegroundWindow() == window;
        let statistics_status = chain.GetFrameStatistics(&mut display).err().map_or(0, |error| error.code().0);
        let queued = chain.GetLastPresentCount().unwrap_or_default();
        listener(FrameStatistics { qpc, foreground, queued, present_status, statistics_status, display });
    }
}
