use crate::{
    audio::Command,
    browser::{Browser, Reveal, music_home},
    catalog::{Catalog, CatalogEvent, Targets},
    location::LocationBar,
    pane::{Kind, Pane},
    player::{Player, PlayerEvent},
    profile::{self, Profile, SavedQueue, WindowPrefs},
    queue_view::QueueView,
    sidebar::{Sidebar, SidebarEvent},
    store::Scope,
    system::{System, SystemEvent},
    theme::materials,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        button::*,
        dock::*,
        input::Input,
        menu::{DropdownMenu, PopupMenuItem},
        *,
    },
    prelude::*,
    *,
};
use std::{path::PathBuf, time::Duration};

gpui_kit::actions!(
    disqro,
    [
        OpenFolder,
        FindTrack,
        PlayPause,
        ToggleWaveform,
        PlaySelected,
        OpenDirectory,
        GoBack,
        GoForward,
        GoUp,
        FocusLocation,
        Refresh,
        NextTrack,
        PreviousTrack,
        ShowQueue,
        HideToTray,
        Quit,
        ToggleLike,
        ActivateTrackLike,
        RateOne,
        RateTwo,
        RateThree,
        RateFour,
        RateFive,
        ClearRating,
        ExtendUp,
        ExtendDown,
        MovePlaylistUp,
        MovePlaylistDown,
        RemoveFromPlaylist,
        NewPlaylist,
        AddMusicFolder
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-w", ToggleWaveform, Some("Disqro && !Input")),
        KeyBinding::new("ctrl-o", OpenFolder, Some("Disqro")),
        KeyBinding::new("ctrl-f", FindTrack, Some("Disqro")),
        KeyBinding::new("ctrl-l", FocusLocation, Some("Disqro")),
        KeyBinding::new("alt-d", FocusLocation, Some("Disqro")),
        KeyBinding::new("f4", FocusLocation, Some("Disqro")),
        KeyBinding::new("space", PlayPause, Some("Disqro && !Input")),
        KeyBinding::new("enter", PlaySelected, Some("DataTable")),
        KeyBinding::new("enter", ActivateTrackLike, Some("TrackLike")),
        KeyBinding::new("space", ActivateTrackLike, Some("TrackLike")),
        KeyBinding::new("enter", OpenDirectory, Some("Tree")),
        KeyBinding::new("alt-left", GoBack, Some("Disqro")),
        KeyBinding::new("backspace", GoBack, Some("Disqro && !Input")),
        KeyBinding::new("alt-right", GoForward, Some("Disqro")),
        KeyBinding::new("alt-up", GoUp, Some("Disqro && !Queue")),
        KeyBinding::new("f5", Refresh, Some("Disqro")),
        KeyBinding::new("ctrl-right", NextTrack, Some("Disqro && !Input")),
        KeyBinding::new("ctrl-left", PreviousTrack, Some("Disqro && !Input")),
        KeyBinding::new("ctrl-q", Quit, Some("Disqro")),
        KeyBinding::new("alt-l", ToggleLike, Some("Disqro && !Input")),
        KeyBinding::new("alt-1", RateOne, Some("Disqro && !Input")),
        KeyBinding::new("alt-2", RateTwo, Some("Disqro && !Input")),
        KeyBinding::new("alt-3", RateThree, Some("Disqro && !Input")),
        KeyBinding::new("alt-4", RateFour, Some("Disqro && !Input")),
        KeyBinding::new("alt-5", RateFive, Some("Disqro && !Input")),
        KeyBinding::new("alt-0", ClearRating, Some("Disqro && !Input")),
        KeyBinding::new(
            "delete",
            RemoveFromPlaylist,
            Some("MusicTracks > DataTable"),
        ),
        KeyBinding::new("shift-up", ExtendUp, Some("MusicTracks > DataTable")),
        KeyBinding::new("shift-down", ExtendDown, Some("MusicTracks > DataTable")),
        KeyBinding::new(
            "ctrl-shift-up",
            MovePlaylistUp,
            Some("MusicTracks > DataTable"),
        ),
        KeyBinding::new(
            "ctrl-shift-down",
            MovePlaylistDown,
            Some("MusicTracks > DataTable"),
        ),
        KeyBinding::new("ctrl-n", NewPlaylist, Some("Disqro")),
    ]);
}

pub struct Workspace {
    browser: Entity<Browser>,
    catalog: Entity<Catalog>,
    player: Entity<Player>,
    sidebar: Entity<Sidebar>,
    location: Entity<LocationBar>,
    dock: Entity<DockArea>,
    panels: Vec<Entity<Pane>>,
    profile: Option<std::sync::Arc<profile::ProfileFile>>,
    save_revision: u64,
    quitting: bool,
    save_dirty: bool,
    window: AnyWindowHandle,
    prefs: WindowPrefs,
    system: Option<System>,
    hwnd: isize,
    hidden_to_tray: bool,
    save_pending: bool,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

const LAYOUT_VERSION: usize = 2;

fn native_handle(window: &Window) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

impl Workspace {
    pub fn new(arg: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (file, mut saved, profile_notice) = match profile::location() {
            Some(path) => {
                let (file, saved, notice) = profile::ProfileFile::open(path);
                (Some(std::sync::Arc::new(file)), saved, notice)
            }
            None => (
                None,
                Profile::default(),
                Some("Workspace storage is unavailable.".into()),
            ),
        };
        let volume = saved.volume.clamp(0., 1.);
        let player = cx.new(|cx| Player::new(volume, saved.full_cd, window, cx));
        let catalog = cx.new(Catalog::new);
        let columns = saved
            .table_layouts
            .take()
            .map(crate::table::Layouts::validate)
            .unwrap_or_else(|| {
                crate::table::Layouts::migrate(&saved.columns, saved.columns_version)
            });
        let browser = cx.new(|cx| {
            Browser::new(
                player.clone(),
                catalog.clone(),
                columns,
                saved.collection,
                window,
                cx,
            )
        });
        let sidebar = cx.new(|cx| {
            Sidebar::new(
                browser.clone(),
                catalog.clone(),
                saved.places.take(),
                std::mem::take(&mut saved.sidebar),
                window,
                cx,
            )
        });
        let queue = cx.new(|cx| QueueView::new(player.clone(), cx));
        let location_bar = cx.new(|cx| LocationBar::new(browser.clone(), window, cx));
        let panels = Kind::ALL
            .into_iter()
            .map(|kind| {
                let pane = cx.new(|cx| {
                    match kind {
                        Kind::Tracks => cx.observe(&browser, |_, _, cx| cx.notify()).detach(),
                        Kind::Queue => cx
                            .subscribe(&player, |_, _, e, cx| {
                                if matches!(e, PlayerEvent::QueueChanged) {
                                    cx.notify()
                                }
                            })
                            .detach(),
                        _ => {}
                    }
                    Pane {
                        kind,
                        browser: browser.clone(),
                        player: player.clone(),
                        sidebar: sidebar.clone(),
                        queue: queue.clone(),
                        focus: cx.focus_handle(),
                    }
                });
                let factory = pane.clone();
                register_panel(cx, kind.key(), move |_, _, _| panel_handle(factory.clone()));
                pane
            })
            .collect::<Vec<_>>();
        let (dock, _) = DockSkin::dock_area("listening", Some(LAYOUT_VERSION), window, cx);
        let (events_tx, events_rx) = async_channel::unbounded();
        let system = System::new(events_tx, player.read(cx).sender());
        let mut subscriptions = vec![
            cx.subscribe(
                &browser,
                |s: &mut Self, _, _: &crate::browser::ViewChanged, cx| s.schedule_save(cx),
            ),
            cx.subscribe(&sidebar, |s: &mut Self, _, e, cx| match e {
                SidebarEvent::Changed => s.schedule_save(cx),
            }),
            cx.subscribe_in(&catalog, window, |_, _, e, window, cx| {
                if let CatalogEvent::Notice(text) = e {
                    window.push_notification(text.clone(), cx);
                }
            }),
            cx.subscribe_in(&player, window, |s: &mut Self, _, e, w, cx| match e {
                PlayerEvent::Locate(track, dest) => {
                    s.show(Kind::Tracks, w, cx);
                    s.browser
                        .update(cx, |b, cx| b.locate(track.clone(), *dest, w, cx));
                }
                PlayerEvent::ContinueCd(entry) => s
                    .browser
                    .update(cx, |b, cx| b.continue_cd(entry.clone(), cx)),
                PlayerEvent::QueueChanged | PlayerEvent::TrackChanged => {
                    s.update_system(cx);
                    s.schedule_save(cx);
                }
                PlayerEvent::StateChanged => {
                    s.update_system(cx);
                    s.schedule_save(cx);
                }
                PlayerEvent::PlaySelection => {}
            }),
            cx.observe_window_visibility(window, |s: &mut Self, visibility, _, cx| {
                let visible = visibility == WindowVisibility::Visible;
                s.player.update(cx, |p, cx| p.set_visible(visible, cx));
                if visible {
                    s.hidden_to_tray = false;
                }
            }),
        ];
        subscriptions.push(cx.observe(&browser, |_, _, cx| cx.notify()));
        let mut app = Self {
            browser,
            catalog,
            player,
            sidebar,
            location: location_bar,
            dock,
            panels,
            profile: file,
            save_revision: 0,
            quitting: false,
            save_dirty: false,
            window: window.window_handle(),
            prefs: saved.window,
            system,
            hwnd: native_handle(window),
            hidden_to_tray: false,
            save_pending: false,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        if !saved.layout.take().is_some_and(|layout| {
            layout.version == Some(LAYOUT_VERSION)
                && app
                    .dock
                    .update(cx, |d, cx| d.load(layout, window, cx))
                    .is_ok()
        }) {
            app.reset(window, cx);
        }
        if let Some(saved) = saved.queue.take()
            && !saved.queue.entries.is_empty()
        {
            app.player.update(cx, |p, cx| {
                p.command(Command::Restore(Box::new(saved.queue), saved.position), cx)
            });
        }
        let opened_file = arg.is_some();
        let target = arg.or(saved.folder).unwrap_or_else(music_home);
        let is_file = crate::library::is_audio(&target)
            || target
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("cue"));
        let folder = if is_file {
            target
                .parent()
                .map(|p| p.to_owned())
                .unwrap_or_else(music_home)
        } else {
            target.clone()
        };
        let restored = saved
            .location
            .take()
            .filter(|l| !l.is_folder() && !opened_file);
        app.browser.update(cx, |b, cx| match restored {
            Some(location) => {
                b.path = folder;
                b.show(location, window, cx);
            }
            None => b.load(folder, is_file.then_some(Reveal::Play(target)), window, cx),
        });
        app.browser.read(cx).list_focus(cx).focus(window, cx);
        if let Some(notice) = profile_notice {
            // The component notification layer is installed after the workspace constructor.
            window.on_next_frame(move |window, cx| window.push_notification(notice, cx));
        }
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events_rx.recv().await {
                if this
                    .update_in(cx, |s, w, cx| s.system_event(event, w, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |s, cx| {
                if s.prefs.close_to_tray && s.tray_available() {
                    s.hide_to_tray(window, cx);
                    false
                } else {
                    s.quit(window, cx);
                    false
                }
            })
            .unwrap_or(true)
        });
        app
    }

    fn snapshot(&self, cx: &App) -> Profile {
        let b = self.browser.read(cx);
        let p = self.player.read(cx);
        let queue = p.queue();
        Profile {
            folder: Some(b.path.clone()),
            volume: p.saved_volume(),
            full_cd: p.full_cd,
            columns: vec![],
            table_layouts: Some(b.table.read(cx).delegate().layouts.clone()),
            layout: Some(self.dock.read(cx).dump(cx)),
            places: Some(self.sidebar.read(cx).places.state()),
            sidebar: self.sidebar.read(cx).state(cx),
            queue: (!queue.entries.is_empty()).then(|| SavedQueue {
                queue: (**queue).clone(),
                position: p.status.position,
            }),
            window: self.prefs,
            collection: b.prefs.clone(),
            location: Some(b.location.clone()),
            columns_version: 0,
        }
    }
    /// What a library action applies to: the table's selection, or the
    /// playing track when nothing is selected.
    fn targets(&self, cx: &App) -> Targets {
        let t = self.browser.read(cx).table.read(cx);
        let targets = t.delegate().targets(None, t.selected_row());
        if !targets.is_empty()
            || t.selected_row()
                .is_some_and(|i| t.delegate().header(i).is_some())
        {
            return targets;
        }
        match &self.player.read(cx).status.track {
            Some(track) => Targets::Tracks(vec![track.clone()]),
            None => targets,
        }
    }
    fn extend_selection(&self, direction: isize, cx: &mut App) {
        self.browser.read(cx).table.clone().update(cx, |t, cx| {
            let primary = t.selected_row();
            if let Some(row) = t.delegate_mut().extend_selection(primary, direction) {
                t.clear_selection(cx);
                t.scroll_to_row(row, cx);
            }
        });
    }
    fn move_selection(&self, down: bool, cx: &mut App) {
        self.browser.read(cx).table.clone().update(cx, |t, cx| {
            t.delegate().move_selection(t.selected_row(), down, cx)
        });
    }

    fn rate(&mut self, stars: u8, cx: &mut Context<Self>) {
        let targets = self.targets(cx);
        self.catalog.update(cx, |c, cx| c.rate(targets, stars, cx));
    }
    fn add_music_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add to Library".into()),
        });
        let catalog = self.catalog.clone();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(paths))) = result.await
                && let Some(path) = paths.into_iter().next()
            {
                catalog.update(cx, |c, cx| c.add_root(path, cx));
            }
        })
        .detach();
    }
    /// Keep at most one debounced write in flight. A change during the write
    /// asks for another snapshot after it completes.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if self.profile.is_none() || self.quitting {
            return;
        }
        self.save_dirty = true;
        if self.save_pending {
            return;
        }
        self.save_pending = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                let Ok(Some((file, revision, snapshot))) = this.update(cx, |s, cx| {
                    if s.quitting {
                        return None;
                    }
                    s.save_dirty = false;
                    s.save_revision += 1;
                    s.profile
                        .clone()
                        .map(|file| (file, s.save_revision, s.snapshot(cx)))
                }) else {
                    return;
                };
                let result = cx
                    .background_spawn(async move { file.commit(revision, &snapshot) })
                    .await;
                let again = this.update(cx, |s, cx| {
                    if let Err(e) = result {
                        let _ = s.window.update(cx, |_, w, cx| {
                            w.push_notification(format!("Workspace save failed: {e}"), cx)
                        });
                    }
                    s.save_pending = s.save_dirty && !s.quitting;
                    s.save_pending
                });
                if !again.unwrap_or(false) {
                    return;
                }
            }
        })
        .detach();
    }
    fn update_system(&mut self, cx: &mut Context<Self>) {
        let Some(system) = &mut self.system else {
            return;
        };
        let p = self.player.read(cx);
        system.update(p.now_playing().as_ref(), &p.status);
    }
    fn system_event(&mut self, e: SystemEvent, window: &mut Window, cx: &mut Context<Self>) {
        let send = |s: &Self, c: Command, cx: &mut Context<Self>| {
            s.player.update(cx, |p, cx| p.command(c, cx))
        };
        match e {
            SystemEvent::TrayUnavailable => {
                if self.hidden_to_tray {
                    self.show_window(window, cx);
                }
                cx.notify();
            }
            SystemEvent::ShowWindow => self.show_window(window, cx),
            SystemEvent::PlayPause => self.play_pause(cx),
            SystemEvent::Previous => send(self, Command::Step(-1), cx),
            SystemEvent::Next => send(self, Command::Step(1), cx),
            SystemEvent::ShowQueue => {
                self.show_window(window, cx);
                self.show(Kind::Queue, window, cx);
            }
            SystemEvent::Quit => self.quit(window, cx),
        }
    }
    fn show_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::system::show_window(self.hwnd);
        window.activate_window();
        self.hidden_to_tray = false;
        cx.notify();
    }
    fn tray_available(&self) -> bool {
        self.system.as_ref().is_some_and(System::tray_available)
    }
    fn hide_to_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.system.as_ref().is_some_and(System::confirm_tray) {
            window.minimize_window();
            return;
        }
        self.hidden_to_tray = true;
        crate::system::hide_window(self.hwnd);
        self.player.update(cx, |p, cx| p.set_visible(false, cx));
        self.schedule_save(cx);
    }
    fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        let Some(file) = self.profile.clone() else {
            cx.quit();
            return;
        };
        self.quitting = true;
        self.save_revision += 1;
        let revision = self.save_revision;
        let mut snapshot = self.snapshot(cx);
        let commands = self.player.read(cx).sender();
        // All personal edits issued before Quit must reach SQLite before process exit.
        let flushed = self.catalog.read(cx).handle().map(|s| s.drain());
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if let Some(flushed) = flushed {
                        flushed.await?;
                    }
                    snapshot.queue = commands.snapshot()?;
                    file.commit(revision, &snapshot)
                })
                .await;
            if let Err(error) = result {
                let Ok(answer) = this.update_in(cx, |s, w, cx| {
                    if s.hidden_to_tray {
                        s.show_window(w, cx);
                    }
                    w.prompt(
                        PromptLevel::Warning,
                        "The workspace could not be saved.",
                        Some(&error.to_string()),
                        &["Keep open", "Quit without saving"],
                        cx,
                    )
                }) else {
                    return;
                };
                if answer.await != Ok(1) {
                    let _ = this.update(cx, |s, _| s.quitting = false);
                    return;
                }
            }
            let _ = this.update(cx, |s, cx| {
                s.system = None;
                cx.quit();
            });
        })
        .detach();
    }
    fn reset(&mut self, w: &mut Window, cx: &mut App) {
        let tab = |k: Kind| {
            DockLayout::tabs().panel_view(panel_handle(self.panels[k.index()].clone()), cx)
        };
        let middle = DockLayout::h_split()
            .child(tab(Kind::Folders), Some(px(232.)))
            .child(tab(Kind::Tracks), None)
            .child(tab(Kind::Queue), Some(px(300.)));
        let layout = DockLayout::v_split()
            .child(middle, None)
            .child(tab(Kind::Player), Some(px(104.)));
        self.dock.update(cx, |d, cx| d.set_center(layout, w, cx));
    }
    fn show(&self, kind: Kind, w: &mut Window, cx: &mut App) {
        let pane = &self.panels[kind.index()];
        let panel = panel_handle(pane.clone());
        self.dock.update(cx, |d, cx| {
            let id = panel.panel_id(cx);
            if d.panel(id).is_some() {
                d.select_panel(id, w, cx);
            } else {
                d.add_panel_view(panel, DockPlacement::Center, None, w, cx);
            }
        });
        pane.read(cx).focus_handle(cx).focus(w, cx);
    }
    fn play_selected(&self, cx: &mut App) {
        let t = self.browser.read(cx).table.read(cx);
        if let Some(ix) = t
            .selected_row()
            .filter(|&i| t.delegate().header(i).is_none())
            .or_else(|| (!t.delegate().is_empty()).then(|| t.delegate().first_track()))
        {
            self.browser
                .read(cx)
                .table
                .clone()
                .update(cx, |t, cx| t.delegate().play(ix, cx));
        }
    }
    fn play_pause(&self, cx: &mut App) {
        let p = self.player.read(cx);
        if p.status.track.is_none() && p.queue().entries.is_empty() {
            self.play_selected(cx);
        } else {
            self.player
                .update(cx, |p, cx| p.command(Command::Toggle, cx));
        }
    }
    fn workspace_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let shell = cx.weak_entity();
        let prefs = self.prefs;
        let tray = self.tray_available();
        Button::new("workspace")
            .ghost()
            .small()
            .icon(Icon::new(IconName::Ellipsis))
            .accessibility_label("Workspace")
            .tooltip("Workspace")
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu
                    .menu("Open Folder…", Box::new(OpenFolder))
                    .menu("Add Music Folder to Library…", Box::new(AddMusicFolder))
                    .menu("New Playlist", Box::new(NewPlaylist))
                    .separator();
                for kind in [Kind::Folders, Kind::Tracks, Kind::Queue, Kind::Player] {
                    let s = shell.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("Show {}", kind.name())).on_click(
                            move |_, w, cx| {
                                let _ = s.update(cx, |s, cx| s.show(kind, w, cx));
                            },
                        ),
                    );
                }
                let (s1, s3, s4, s5) = (shell.clone(), shell.clone(), shell.clone(), shell.clone());
                menu.item(
                    PopupMenuItem::new("Restore Default Layout").on_click(move |_, w, cx| {
                        let _ = s1.update(cx, |s, cx| s.reset(w, cx));
                    }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Close to Tray")
                        .checked(prefs.close_to_tray)
                        .disabled(!tray)
                        .on_click(move |_, _, cx| {
                            let _ = s3.update(cx, |s, cx| {
                                s.prefs.close_to_tray = !s.prefs.close_to_tray;
                                s.schedule_save(cx);
                                cx.notify();
                            });
                        }),
                )
                .separator()
                .item(PopupMenuItem::new("Hide to Tray").disabled(!tray).on_click(
                    move |_, w, cx| {
                        let _ = s4.update(cx, |s, cx| s.hide_to_tray(w, cx));
                    },
                ))
                .item(PopupMenuItem::new("Quit Disqro").on_click(move |_, w, cx| {
                    let _ = s5.update(cx, |s, cx| s.quit(w, cx));
                }))
            })
    }
    fn nav_button(
        id: &'static str,
        icon: IconName,
        label: &'static str,
        enabled: bool,
        cx: &Context<Self>,
    ) -> Button {
        let m = materials(cx);
        Button::new(id)
            .ghost()
            .small()
            .icon(Icon::new(icon).size(px(16.)).text_color(m.text_muted))
            .accessibility_label(label)
            .tooltip(label)
            .disabled(!enabled)
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let b = self.browser.read(cx);
        let (can_back, can_forward) = (b.can_go_back(), b.can_go_forward());
        let has_parent = b.can_go_up();
        let search = b.search.clone();
        let nav = h_flex()
            .occlude()
            .flex_shrink_0()
            .gap(px(2.))
            .child(
                Self::nav_button("back", IconName::ArrowLeft, "Back · Alt+Left", can_back, cx)
                    .on_click(
                        cx.listener(|s, _, w, cx| s.browser.update(cx, |b, cx| b.back(w, cx))),
                    ),
            )
            .child(
                Self::nav_button(
                    "forward",
                    IconName::ArrowRight,
                    "Forward · Alt+Right",
                    can_forward,
                    cx,
                )
                .on_click(
                    cx.listener(|s, _, w, cx| s.browser.update(cx, |b, cx| b.forward(w, cx))),
                ),
            )
            .child(
                Self::nav_button(
                    "up",
                    IconName::ArrowUp,
                    "Parent folder · Alt+Up",
                    has_parent,
                    cx,
                )
                .on_click(cx.listener(|s, _, w, cx| s.browser.update(cx, |b, cx| b.up(w, cx)))),
            );
        let bar = h_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .gap(px(8.))
            .pr(px(6.))
            .items_center()
            .child(
                img("brand/compact.png")
                    .size(px(18.))
                    .flex_shrink_0()
                    .mr(px(4.)),
            )
            .child(nav)
            .child(
                div()
                    .occlude()
                    .flex_basis(relative(1.))
                    .min_w(px(160.))
                    .max_w(px(900.))
                    .child(self.location.clone()),
            )
            // The space between is the caption: drag to move the window.
            .child(div().flex_1().min_w(px(24.)).h_full())
            .child(
                div().occlude().flex_shrink_0().w(px(220.)).child(
                    Input::new(&search)
                        .small()
                        .prefix(
                            Icon::new(IconName::Search)
                                .size(px(14.))
                                .text_color(m.text_faint),
                        )
                        .cleanable(true)
                        .aria_label("Find tracks"),
                ),
            )
            .child(div().occlude().child(self.workspace_menu(cx)));

        v_flex()
            .key_context("Disqro")
            .track_focus(&self.focus)
            .size_full()
            .bg(m.canvas)
            .text_color(m.text)
            .on_action(
                cx.listener(|s, _: &OpenFolder, w, cx| {
                    s.browser.update(cx, |b, cx| b.choose(w, cx))
                }),
            )
            .on_action(
                cx.listener(|s, _: &GoBack, w, cx| s.browser.update(cx, |b, cx| b.back(w, cx))),
            )
            .on_action(
                cx.listener(|s, _: &GoForward, w, cx| {
                    s.browser.update(cx, |b, cx| b.forward(w, cx))
                }),
            )
            .on_action(cx.listener(|s, _: &GoUp, w, cx| s.browser.update(cx, |b, cx| b.up(w, cx))))
            .on_action(
                cx.listener(|s, _: &Refresh, w, cx| s.browser.update(cx, |b, cx| b.reload(w, cx))),
            )
            .on_action(cx.listener(|s, _: &FocusLocation, w, cx| {
                s.location.update(cx, |l, cx| l.edit(w, cx))
            }))
            .on_action(cx.listener(|s, _: &FindTrack, w, cx| {
                let search = s.browser.read(cx).search.clone();
                search.update(cx, |i, cx| {
                    i.focus(w, cx);
                    i.select_all(w, cx);
                });
            }))
            .on_action(cx.listener(|s, _: &ToggleWaveform, _, cx| {
                s.player
                    .update(cx, |p, cx| p.set_perspective(!p.full_cd, cx))
            }))
            .on_action(cx.listener(|s, _: &PlayPause, _, cx| s.play_pause(cx)))
            .on_action(cx.listener(|s, _: &NextTrack, _, cx| {
                s.player.update(cx, |p, cx| p.command(Command::Step(1), cx))
            }))
            .on_action(cx.listener(|s, _: &PreviousTrack, _, cx| {
                s.player
                    .update(cx, |p, cx| p.command(Command::Step(-1), cx))
            }))
            .on_action(cx.listener(|s, _: &ShowQueue, w, cx| s.show(Kind::Queue, w, cx)))
            .on_action(cx.listener(|s, _: &ToggleLike, _, cx| {
                let targets = s.targets(cx);
                s.catalog.update(cx, |c, cx| c.toggle_like(targets, cx));
            }))
            .on_action(cx.listener(|s, _: &RateOne, _, cx| s.rate(1, cx)))
            .on_action(cx.listener(|s, _: &RateTwo, _, cx| s.rate(2, cx)))
            .on_action(cx.listener(|s, _: &RateThree, _, cx| s.rate(3, cx)))
            .on_action(cx.listener(|s, _: &RateFour, _, cx| s.rate(4, cx)))
            .on_action(cx.listener(|s, _: &RateFive, _, cx| s.rate(5, cx)))
            .on_action(cx.listener(|s, _: &ClearRating, _, cx| s.rate(0, cx)))
            .on_action(cx.listener(|s, _: &ExtendUp, _, cx| s.extend_selection(-1, cx)))
            .on_action(cx.listener(|s, _: &ExtendDown, _, cx| s.extend_selection(1, cx)))
            .on_action(cx.listener(|s, _: &MovePlaylistUp, _, cx| s.move_selection(false, cx)))
            .on_action(cx.listener(|s, _: &MovePlaylistDown, _, cx| s.move_selection(true, cx)))
            .on_action(cx.listener(|s, _: &RemoveFromPlaylist, _, cx| {
                let t = s.browser.read(cx).table.read(cx);
                if let Targets::Keys(Scope::Playlist(p), entries) =
                    t.delegate().targets(None, t.selected_row())
                    && !entries.is_empty()
                {
                    s.catalog
                        .update(cx, |c, cx| c.remove_entries(p, entries, cx));
                }
            }))
            .on_action(cx.listener(|s, _: &NewPlaylist, _, cx| {
                s.catalog
                    .update(cx, |c, cx| c.new_playlist(Targets::Tracks(vec![]), cx));
            }))
            .on_action(cx.listener(|s, _: &AddMusicFolder, w, cx| s.add_music_folder(w, cx)))
            .on_action(cx.listener(|s, _: &HideToTray, w, cx| s.hide_to_tray(w, cx)))
            .on_action(cx.listener(|s, _: &Quit, w, cx| s.quit(w, cx)))
            .on_action(cx.listener(|s, _: &PlaySelected, w, cx| {
                if s.browser.read(cx).listing_discs() {
                    s.browser.update(cx, |b, cx| {
                        let ix = b.discs.read(cx).selected_row().unwrap_or(0);
                        b.show_disc(ix, w, cx);
                    });
                } else {
                    s.browser.update(cx, |b, cx| {
                        let t = b.table.read(cx);
                        b.activate(
                            t.selected_row()
                                .unwrap_or_else(|| t.delegate().first_track()),
                            w,
                            cx,
                        );
                    });
                }
            }))
            .on_action(cx.listener(|s, _: &OpenDirectory, w, cx| {
                let b = s.browser.read(cx);
                let folders = b.folders.read(cx);
                let shown = folders.current().map(|(r, p)| (r.to_owned(), p.to_owned()));
                if let Some((root, path)) = folders
                    .state
                    .read(cx)
                    .selected_item()
                    .and_then(|i| folders.entry(&i.id))
                    && (shown.as_ref() != Some(&(root.clone(), path.clone()))
                        || !b.location.is_folder())
                {
                    s.browser.update(cx, |b, cx| b.open_in(path, root, w, cx));
                }
            }))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|s, _, w, cx| s.browser.update(cx, |b, cx| b.back(w, cx))),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|s, _, w, cx| s.browser.update(cx, |b, cx| b.forward(w, cx))),
            )
            .child(TitleBar::new().h(px(40.)).pl(px(12.)).child(bar))
            .child(div().flex_1().min_h_0().child(self.dock.clone()))
    }
}
