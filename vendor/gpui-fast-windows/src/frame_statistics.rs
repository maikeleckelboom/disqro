//! Opt-in DXGI display statistics for native qualification.
use std::sync::OnceLock;
use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::{
        Direct3D11::{ID3D11DeviceContext, ID3D11DeviceContext1, ID3D11RenderTargetView},
        Dxgi::{DXGI_FRAME_STATISTICS, IDXGISwapChain1},
    },
    System::Performance::QueryPerformanceCounter,
    UI::WindowsAndMessaging::GetForegroundWindow,
};
use windows::core::Interface;

pub struct FrameStatistics {
    pub qpc: i64,
    pub foreground: bool,
    pub queued: u32,
    pub present_status: i32,
    pub statistics_status: i32,
    pub display: DXGI_FRAME_STATISTICS,
}

pub struct FrameStage {
    pub qpc: i64,
    pub ticks: i64,
    pub stage: &'static str,
    pub value: u64,
    pub detail: u64,
}

static LISTENER: OnceLock<Box<dyn Fn(FrameStatistics) + Send + Sync>> = OnceLock::new();
static STAGES: OnceLock<Box<dyn Fn(FrameStage) + Send + Sync>> = OnceLock::new();

pub fn listen(listener: impl Fn(FrameStatistics) + Send + Sync + 'static) {
    let _ = LISTENER.set(Box::new(listener));
}

pub fn listen_to_stages(listener: impl Fn(FrameStage) + Send + Sync + 'static) {
    let _ = STAGES.set(Box::new(listener));
}

pub fn qpc() -> i64 {
    let mut value = 0;
    // SAFETY: Windows writes a single counter value to the output slot.
    unsafe { let _ = QueryPerformanceCounter(&mut value); }
    value
}

pub fn start() -> Option<i64> {
    STAGES.get().map(|_| qpc())
}

pub fn stage(start: Option<i64>, stage: &'static str, value: u64, detail: u64) {
    if let (Some(listener), Some(start)) = (STAGES.get(), start) {
        let qpc = qpc();
        listener(FrameStage { qpc, ticks: qpc - start, stage, value, detail });
    }
}

/// Qualifies composed frames with the six-colour Desktop Duplication probe.
pub(crate) fn marker(context: &ID3D11DeviceContext, target: &ID3D11RenderTargetView, height: u32) {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    static ENABLED: OnceLock<bool> = OnceLock::new();
    static FRAME: AtomicUsize = AtomicUsize::new(0);
    if !*ENABLED.get_or_init(|| std::env::var_os("DISQRO_QUALIFY_PRESENT_MARKER").is_some()) {
        return;
    }
    let Ok(context) = context.cast::<ID3D11DeviceContext1>() else { return };
    let colours = [
        [1., 0., 0., 1.], [1., 1., 0., 1.], [0., 1., 0., 1.],
        [0., 1., 1., 1.], [0., 0., 1., 1.], [1., 0., 1., 1.],
    ];
    let bounds = RECT { left: 0, right: 32, top: height.saturating_sub(32) as i32, bottom: height as i32 };
    // SAFETY: the redraw and marker use this swap chain's immediate context.
    unsafe { context.ClearView(target, &colours[FRAME.fetch_add(1, Relaxed) % colours.len()], Some(&[bounds])); }
}

pub(crate) fn record(chain: &IDXGISwapChain1, window: HWND, present_status: i32) {
    let Some(listener) = LISTENER.get() else { return };
    let qpc = qpc();
    let mut display = DXGI_FRAME_STATISTICS::default();
    // SAFETY: the caller owns this window's live swap chain and the output
    // slots are valid for the lifetime of the system calls.
    unsafe {
        let foreground = GetForegroundWindow() == window;
        let statistics_status = chain.GetFrameStatistics(&mut display).err().map_or(0, |error| error.code().0);
        let queued = chain.GetLastPresentCount().unwrap_or_default();
        listener(FrameStatistics { qpc, foreground, queued, present_status, statistics_status, display });
    }
}
