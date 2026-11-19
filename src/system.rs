//! Windows integration: the notification-area icon and the system media
//! transport controls (media keys and the volume flyout).
//!
//! Both drive the same transport as the window, through the application, so
//! there is one playback state. The tray lives on a hidden window owned by the
//! interface thread; GPUI's message loop dispatches its messages.
use crate::{audio::Status, player::NowPlaying};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemEvent {
    ShowWindow,
    PlayPause,
    Previous,
    Next,
    ShowQueue,
    Quit,
}

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub struct System;
    impl System {
        pub fn new(
            _: async_channel::Sender<SystemEvent>,
            _: std::sync::mpsc::Sender<crate::audio::Command>,
        ) -> Option<Self> {
            None
        }
        pub fn update(&mut self, _: Option<&NowPlaying>, _: &Status) {}
    }
    pub fn hide_window(_: isize) {}
    pub fn show_window(_: isize) {}
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::audio::Command;
    use std::{cell::RefCell, sync::mpsc};
    use windows::{
        Foundation::{TimeSpan, TypedEventHandler},
        Media::{
            MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs,
            SystemMediaTransportControls, SystemMediaTransportControlsButton,
            SystemMediaTransportControlsButtonPressedEventArgs,
            SystemMediaTransportControlsTimelineProperties,
        },
        Storage::Streams::{DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference},
        Win32::{
            Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
            System::{
                LibraryLoader::GetModuleHandleW, WinRT::ISystemMediaTransportControlsInterop,
            },
            UI::{
                Controls::{LIM_SMALL, LoadIconMetric},
                Shell::{
                    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
                    NIM_SETVERSION, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
                },
                WindowsAndMessaging::{
                    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
                    DestroyMenu, DestroyWindow, GetCursorPos, HICON, IsIconic, MF_GRAYED,
                    MF_SEPARATOR, MF_STRING, PostMessageW, RegisterClassExW,
                    RegisterWindowMessageW, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow,
                    SetMenuDefaultItem, ShowWindow, TPM_BOTTOMALIGN, TPM_RETURNCMD,
                    TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_APP, WM_CONTEXTMENU, WM_LBUTTONDBLCLK,
                    WM_LBUTTONUP, WM_NULL, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_POPUP,
                },
            },
        },
        core::{HSTRING, PCWSTR, w},
    };

    const CALLBACK: u32 = WM_APP + 1;
    const NIN_SELECT: u32 = 0x400;
    const NIN_KEYSELECT: u32 = 0x401;

    struct TrayState {
        events: async_channel::Sender<SystemEvent>,
        taskbar_created: u32,
        icon: HICON,
        tip: String,
        playing: bool,
        has_track: bool,
    }
    thread_local! {
        static TRAY: RefCell<Option<TrayState>> = const { RefCell::new(None) };
    }

    fn notify_data(hwnd: HWND, icon: HICON, tip: &str) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: CALLBACK,
            hIcon: icon,
            ..Default::default()
        };
        for (slot, c) in data.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
            *slot = c;
        }
        data
    }
    unsafe fn add_icon(hwnd: HWND, icon: HICON, tip: &str) {
        let mut data = notify_data(hwnd, icon, tip);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &data);
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
    }

    fn send(event: SystemEvent) {
        TRAY.with(|t| {
            if let Some(t) = t.borrow().as_ref() {
                let _ = t.events.try_send(event);
            }
        });
    }

    fn show_menu(hwnd: HWND) {
        // Copy the state out: the menu loop below re-enters this procedure.
        let Some((playing, has_track)) =
            TRAY.with(|t| t.borrow().as_ref().map(|t| (t.playing, t.has_track)))
        else {
            return;
        };
        const SHOW: usize = 1;
        const TOGGLE: usize = 2;
        const PREVIOUS: usize = 3;
        const NEXT: usize = 4;
        const QUEUE: usize = 5;
        const QUIT: usize = 6;
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let item = |id: usize, label: &str, enabled: bool| {
                let flags = if enabled {
                    MF_STRING
                } else {
                    MF_STRING | MF_GRAYED
                };
                let _ = AppendMenuW(menu, flags, id, &HSTRING::from(label));
            };
            item(SHOW, "Show Disqro", true);
            let _ = SetMenuDefaultItem(menu, SHOW as u32, 0);
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            item(TOGGLE, if playing { "Pause" } else { "Play" }, true);
            item(PREVIOUS, "Previous", has_track);
            item(NEXT, "Next", has_track);
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            item(QUEUE, "Show Queue", true);
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            item(QUIT, "Quit Disqro", true);
            let mut at = POINT::default();
            let _ = GetCursorPos(&mut at);
            // Without this the menu does not close when focus moves elsewhere.
            let _ = SetForegroundWindow(hwnd);
            let chosen = TrackPopupMenuEx(
                menu,
                (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN).0,
                at.x,
                at.y,
                hwnd,
                None,
            )
            .0 as usize;
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            match chosen {
                SHOW => send(SystemEvent::ShowWindow),
                TOGGLE => send(SystemEvent::PlayPause),
                PREVIOUS => send(SystemEvent::Previous),
                NEXT => send(SystemEvent::Next),
                QUEUE => send(SystemEvent::ShowQueue),
                QUIT => send(SystemEvent::Quit),
                _ => {}
            }
        }
    }

    unsafe extern "system" fn tray_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if msg == CALLBACK {
            match (l.0 as u32) & 0xffff {
                NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                    send(SystemEvent::ShowWindow)
                }
                WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            return LRESULT(0);
        }
        let restart = TRAY.with(|t| {
            t.borrow()
                .as_ref()
                .filter(|t| t.taskbar_created != 0 && t.taskbar_created == msg)
                .map(|t| (t.icon, t.tip.clone()))
        });
        if let Some((icon, tip)) = restart {
            // Explorer restarted; the notification area forgot the icon.
            unsafe { add_icon(hwnd, icon, &tip) };
            return LRESULT(0);
        }
        unsafe { DefWindowProcW(hwnd, msg, w, l) }
    }

    struct Media {
        smtc: SystemMediaTransportControls,
        shown: Option<NowPlaying>,
        status: Option<MediaPlaybackStatus>,
        timeline: (u64, u64),
    }
    fn span(seconds: f64) -> TimeSpan {
        TimeSpan {
            Duration: (seconds.max(0.) * 10_000_000.) as i64,
        }
    }
    impl Media {
        fn new(hwnd: HWND, tx: mpsc::Sender<Command>) -> windows::core::Result<Self> {
            let interop = windows::core::factory::<
                SystemMediaTransportControls,
                ISystemMediaTransportControlsInterop,
            >()?;
            let smtc: SystemMediaTransportControls = unsafe { interop.GetForWindow(hwnd)? };
            smtc.SetIsEnabled(true)?;
            smtc.SetIsPlayEnabled(true)?;
            smtc.SetIsPauseEnabled(true)?;
            smtc.SetIsNextEnabled(true)?;
            smtc.SetIsPreviousEnabled(true)?;
            smtc.SetIsStopEnabled(true)?;
            let buttons = tx.clone();
            smtc.ButtonPressed(&TypedEventHandler::<
                SystemMediaTransportControls,
                SystemMediaTransportControlsButtonPressedEventArgs,
            >::new(move |_, args| {
                let command = match args.ok()?.Button()? {
                    SystemMediaTransportControlsButton::Play => Command::SetPlaying(true),
                    SystemMediaTransportControlsButton::Pause => Command::SetPlaying(false),
                    SystemMediaTransportControlsButton::Next => Command::Step(1),
                    SystemMediaTransportControlsButton::Previous => Command::Step(-1),
                    SystemMediaTransportControlsButton::Stop => Command::Stop,
                    _ => return Ok(()),
                };
                let _ = buttons.send(command);
                Ok(())
            }))?;
            smtc.PlaybackPositionChangeRequested(&TypedEventHandler::<
                SystemMediaTransportControls,
                PlaybackPositionChangeRequestedEventArgs,
            >::new(move |_, args| {
                let at = args.ok()?.RequestedPlaybackPosition()?.Duration as f64 / 10_000_000.;
                let _ = tx.send(Command::SeekTo(at));
                Ok(())
            }))?;
            smtc.SetPlaybackStatus(MediaPlaybackStatus::Closed)?;
            Ok(Self {
                smtc,
                shown: None,
                status: None,
                timeline: (u64::MAX, u64::MAX),
            })
        }
        fn update(&mut self, now: Option<&NowPlaying>, s: &Status) -> windows::core::Result<()> {
            let status = match (&s.track, s.playing) {
                (None, _) => MediaPlaybackStatus::Closed,
                (Some(_), true) => MediaPlaybackStatus::Playing,
                (Some(_), false) => MediaPlaybackStatus::Paused,
            };
            if self.status != Some(status) {
                self.smtc.SetPlaybackStatus(status)?;
                self.status = Some(status);
            }
            self.smtc.SetIsNextEnabled(s.next)?;
            self.smtc.SetIsPreviousEnabled(s.previous)?;
            if now != self.shown.as_ref() {
                let updater = self.smtc.DisplayUpdater()?;
                updater.ClearAll()?;
                if let Some(now) = now {
                    updater.SetType(MediaPlaybackType::Music)?;
                    let music = updater.MusicProperties()?;
                    music.SetTitle(&HSTRING::from(now.title.as_str()))?;
                    music.SetArtist(&HSTRING::from(now.artist.as_str()))?;
                    music.SetAlbumTitle(&HSTRING::from(now.album.as_str()))?;
                    if let Some(png) = &now.cover {
                        let stream = InMemoryRandomAccessStream::new()?;
                        let writer = DataWriter::CreateDataWriter(&stream)?;
                        writer.WriteBytes(png)?;
                        writer.StoreAsync()?.join()?;
                        writer.DetachStream()?;
                        stream.Seek(0)?;
                        updater.SetThumbnail(&RandomAccessStreamReference::CreateFromStream(
                            &stream,
                        )?)?;
                    }
                }
                updater.Update()?;
                self.shown = now.cloned();
            }
            // Windows interpolates the position itself; report it when it
            // jumps, when play state changes, or when the track changes.
            let timeline = (
                (s.duration * 10.) as u64,
                (s.position * 10.) as u64 / if s.playing { 50 } else { 1 },
            );
            if timeline != self.timeline {
                self.timeline = timeline;
                let props = SystemMediaTransportControlsTimelineProperties::new()?;
                props.SetStartTime(span(0.))?;
                props.SetMinSeekTime(span(0.))?;
                props.SetEndTime(span(s.duration))?;
                props.SetMaxSeekTime(span(s.duration))?;
                props.SetPosition(span(s.position))?;
                self.smtc.UpdateTimelineProperties(&props)?;
            }
            Ok(())
        }
    }

    pub struct System {
        hwnd: HWND,
        icon: HICON,
        media: Option<Media>,
        tip: String,
    }

    impl System {
        pub fn new(
            events: async_channel::Sender<SystemEvent>,
            transport: mpsc::Sender<Command>,
        ) -> Option<Self> {
            unsafe {
                let instance = GetModuleHandleW(None).ok()?;
                let class = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(tray_proc),
                    hInstance: instance.into(),
                    lpszClassName: w!("DisqroTray"),
                    ..Default::default()
                };
                RegisterClassExW(&class);
                let hwnd = CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    w!("DisqroTray"),
                    w!("Disqro"),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .ok()?;
                // The executable's icon resource 1, at the notification-area size.
                let icon = LoadIconMetric(Some(instance.into()), PCWSTR(1 as _), LIM_SMALL)
                    .unwrap_or_default();
                let tip = "Disqro".to_owned();
                TRAY.with(|t| {
                    *t.borrow_mut() = Some(TrayState {
                        events,
                        taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
                        icon,
                        tip: tip.clone(),
                        playing: false,
                        has_track: false,
                    })
                });
                add_icon(hwnd, icon, &tip);
                let media = match Media::new(hwnd, transport) {
                    Ok(m) => Some(m),
                    Err(e) => {
                        eprintln!("System media controls are unavailable: {e}");
                        None
                    }
                };
                Some(Self {
                    hwnd,
                    icon,
                    media,
                    tip,
                })
            }
        }
        pub fn update(&mut self, now: Option<&NowPlaying>, status: &Status) {
            TRAY.with(|t| {
                if let Some(t) = t.borrow_mut().as_mut() {
                    t.playing = status.playing;
                    t.has_track = status.track.is_some();
                }
            });
            let tip = match now {
                Some(n) if !n.artist.is_empty() => format!("Disqro\n{} · {}", n.title, n.artist),
                Some(n) => format!("Disqro\n{}", n.title),
                None => "Disqro".into(),
            };
            if tip != self.tip {
                self.tip = tip.clone();
                TRAY.with(|t| {
                    if let Some(t) = t.borrow_mut().as_mut() {
                        t.tip = tip.clone();
                    }
                });
                let data = notify_data(self.hwnd, self.icon, &tip);
                unsafe {
                    let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
                }
            }
            if let Some(media) = &mut self.media
                && let Err(e) = media.update(now, status)
            {
                eprintln!("Media controls update failed: {e}");
            }
        }
    }
    impl Drop for System {
        fn drop(&mut self) {
            unsafe {
                let data = notify_data(self.hwnd, self.icon, "");
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                if !self.icon.is_invalid() {
                    let _ = DestroyIcon(self.icon);
                }
                let _ = DestroyWindow(self.hwnd);
            }
            TRAY.with(|t| t.borrow_mut().take());
        }
    }

    fn hwnd(raw: isize) -> HWND {
        HWND(raw as _)
    }
    /// Hides the main window, taking it off the taskbar.
    pub fn hide_window(raw: isize) {
        unsafe {
            let _ = ShowWindow(hwnd(raw), SW_HIDE);
        }
    }
    pub fn show_window(raw: isize) {
        let h = hwnd(raw);
        unsafe {
            let _ = ShowWindow(
                h,
                if IsIconic(h).as_bool() {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            let _ = SetForegroundWindow(h);
        }
    }
}
