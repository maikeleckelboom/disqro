//! Library: personal music organisation. The indexed collections, Liked
//! Tracks and the playlists, each opened in the active Music Browser.
//!
//! A folder dragged here from Locations is added to the library as a
//! collection: it is indexed where it is, never moved or copied. Tracks
//! dropped on a playlist are added to it as new entries.
use super::{
    Destination, PaneContent, PanelType, Router, Services,
    nav::{DraggedFolder, ROW, nav_row},
};
use crate::{
    audio::Command,
    browser::{Location, ViewChanged},
    catalog::{Catalog, CatalogEvent, Targets},
    player::Player,
    store::{PlaylistId, RootId, Scope},
    table::{playlist_target, queue_targets},
    theme::materials,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, Sizable, WindowExt,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputEvent, InputState},
        menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
        tooltip::Tooltip,
        v_flex,
    },
    prelude::*,
    *,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, rc::Rc, sync::Arc};

pub static TYPE: PanelType = PanelType {
    key: "disqro.library",
    title: "Library",
    icon: IconName::Library,
    instances: super::Instances::Single,
    home: super::Home::Leading,
    build: |services, record, window, cx| {
        let state: LibraryState = record.state();
        let panel = cx.new(|cx| LibraryPanel::new(services, state, window, cx));
        Rc::new(panel)
    },
};

/// What a Library panel keeps between sessions.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryState {
    #[serde(skip_serializing_if = "is_zero")]
    pub scroll: f32,
}
fn is_zero(v: &f32) -> bool {
    *v == 0.
}

pub struct LibraryPanel {
    catalog: Entity<Catalog>,
    router: Entity<Router>,
    player: Entity<Player>,
    renaming: Option<(PlaylistId, Entity<InputState>)>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// Follows the active browser, whose view the rows mark.
    browser: Option<Subscription>,
}

impl LibraryPanel {
    fn new(
        services: &Services,
        state: LibraryState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&services.catalog, |_, _, cx| cx.notify())
            .detach();
        cx.subscribe_in(&services.catalog, window, |s, _, e, window, cx| {
            if let CatalogEvent::Created(id) = e {
                s.rename_playlist(*id, window, cx);
            }
        })
        .detach();
        cx.observe(&services.router, |s: &mut Self, _, cx| {
            s.follow(cx);
            cx.notify();
        })
        .detach();
        let scroll = ScrollHandle::new();
        scroll.set_offset(point(px(0.), px(-state.scroll.max(0.))));
        let mut this = Self {
            catalog: services.catalog.clone(),
            router: services.router.clone(),
            player: services.player.clone(),
            renaming: None,
            scroll,
            focus: cx.focus_handle(),
            browser: None,
        };
        this.follow(cx);
        this
    }
    fn follow(&mut self, cx: &mut Context<Self>) {
        self.browser = self
            .router
            .read(cx)
            .active()
            .map(|browser| cx.subscribe(&browser, |_, _, _: &ViewChanged, cx| cx.notify()));
    }
    fn state(&self) -> LibraryState {
        LibraryState {
            scroll: -f32::from(self.scroll.offset().y),
        }
    }
    /// The view the active browser shows.
    fn location(&self, cx: &App) -> Option<Location> {
        self.router
            .read(cx)
            .active()
            .map(|b| b.read(cx).location.clone())
    }
    fn open(
        router: &Entity<Router>,
        location: Location,
        new_tab: bool,
        w: &mut Window,
        cx: &mut App,
    ) {
        Router::open(router, Destination::View(location), new_tab, w, cx);
    }
    /// Starts editing a playlist's name in place.
    fn rename_playlist(&mut self, id: PlaylistId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.catalog.read(cx).playlist(id).map(|p| p.name.clone()) else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        cx.subscribe_in(&input, window, move |s, input, e, window, cx| match e {
            InputEvent::PressEnter { .. } | InputEvent::Blur
                if s.renaming.as_ref().is_some_and(|(p, _)| *p == id) =>
            {
                let name = input.read(cx).value().to_string();
                s.renaming = None;
                s.catalog
                    .update(cx, |c, cx| c.rename_playlist(id, name, cx));
                if matches!(e, InputEvent::PressEnter { .. }) {
                    s.focus.focus(window, cx);
                }
                cx.notify();
            }
            _ => {}
        })
        .detach();
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.renaming = Some((id, input));
        cx.notify();
    }
    fn confirm_delete_playlist(
        &mut self,
        id: PlaylistId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self.catalog.read(cx).playlist(id).cloned() else {
            return;
        };
        let catalog = self.catalog.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let catalog = catalog.clone();
            alert
                .title(format!("Delete “{}”?", p.name))
                .description(match p.entries {
                    0 => "The playlist is empty. Your music files are not changed.".to_string(),
                    1 => "Its one entry will be removed. Your music files are not changed.".into(),
                    n => format!(
                        "Its {n} entries will be removed. Your music files are not changed."
                    ),
                })
                .show_cancel(true)
                .ok_text("Delete Playlist")
                .on_ok(move |_, _, cx| {
                    catalog.update(cx, |c, cx| c.delete_playlist(id, cx));
                    true
                })
        });
    }
    fn confirm_remove_root(&mut self, id: RootId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.catalog.read(cx).collection(id) else {
            return;
        };
        let name = c.root.name.clone();
        let catalog = self.catalog.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let catalog = catalog.clone();
            alert
                .title(format!("Remove “{name}” from the library?"))
                .description(
                    "Its folder and music files are not changed. Liked tracks, ratings and \
                     playlist entries are kept and reconnect if you add the folder again.",
                )
                .show_cancel(true)
                .ok_text("Remove from Library")
                .on_ok(move |_, _, cx| {
                    catalog.update(cx, |c, cx| c.remove_root(id, cx));
                    true
                })
        });
    }
    fn library_menu(menu: PopupMenu) -> PopupMenu {
        menu.item(
            PopupMenuItem::new("Add Music Folder…")
                .icon(IconName::FolderPlus)
                .action(Box::new(crate::workspace::AddMusicFolder)),
        )
        .item(
            PopupMenuItem::new("New Playlist")
                .icon(IconName::Plus)
                .action(Box::new(crate::workspace::NewPlaylist)),
        )
    }
    /// Open and Open in New Tab for a view.
    fn open_items(
        menu: PopupMenu,
        router: Entity<Router>,
        location: Location,
        icon: IconName,
    ) -> PopupMenu {
        let (r, l) = (router.clone(), location.clone());
        let menu = menu.item(
            PopupMenuItem::new("Open")
                .icon(icon)
                .on_click(move |_, w, cx| Self::open(&r, l.clone(), false, w, cx)),
        );
        Destination::View(location).open_items(menu, router)
    }
    /// A row that opens `location`: in the active browser, or in a new one
    /// with Ctrl.
    fn opens(row: Stateful<Div>, router: &Entity<Router>, location: Location) -> Stateful<Div> {
        let router = router.clone();
        row.on_click(move |e, w, cx| {
            Self::open(
                &router,
                location.clone(),
                e.modifiers().secondary(cx),
                w,
                cx,
            )
        })
    }

    /// The collections, Liked Tracks and the playlists.
    fn render_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let m = *materials(cx);
        let location = self.location(cx);
        let catalog = self.catalog.read(cx);
        let mut rows = vec![];
        {
            let all = Location::Library(None);
            let router = self.router.clone();
            rows.push(
                Self::opens(
                    nav_row(
                        "all-music".into(),
                        Icon::new(IconName::Library),
                        "All Music".into(),
                        matches!(
                            location,
                            Some(
                                Location::Library(None)
                                    | Location::Album { root: None, .. }
                                    | Location::Release { root: None, .. }
                            )
                        ),
                        &m,
                    ),
                    &self.router,
                    all.clone(),
                )
                .context_menu(move |menu, _, _| {
                    let menu =
                        Self::open_items(menu, router.clone(), all.clone(), IconName::Library);
                    Self::library_menu(menu.separator())
                })
                .into_any_element(),
            );
        }
        for c in &catalog.collections {
            let id = c.root.id;
            let path = c.root.path.clone();
            let status = c.status.clone();
            let tip = (!c.online).then(|| format!("{} is not available", path.display()));
            let this = cx.weak_entity();
            let router = self.router.clone();
            let scanning = catalog.is_scanning(id);
            let selected = match location {
                Some(
                    Location::Library(Some(r))
                    | Location::Album { root: Some(r), .. }
                    | Location::Release { root: Some(r), .. },
                ) => r == id,
                _ => false,
            };
            let view = Location::Library(Some(id));
            let row = nav_row(
                ("collection", id as usize).into(),
                Icon::new(IconName::Disc3),
                c.root.name.clone().into(),
                selected,
                &m,
            )
            .when(!c.online, |d| d.opacity(0.45))
            .when_some(status.filter(|_| scanning), |d, status| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(m.text_faint)
                        .child(if status.starts_with("Reading") {
                            "Reading"
                        } else {
                            "Scanning"
                        }),
                )
            })
            .when_some(tip, |d, tip| {
                d.tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
            });
            let row =
                Self::opens(row, &self.router, view.clone()).context_menu(move |menu, _, _| {
                    let (b, c) = (this.clone(), this.clone());
                    let folder = path.clone();
                    let r = router.clone();
                    let menu =
                        Self::open_items(menu, router.clone(), view.clone(), IconName::Library)
                            .item(
                                PopupMenuItem::new("Browse Folder")
                                    .icon(IconName::FolderOpen)
                                    .on_click(move |_, w, cx| {
                                        let destination = Destination::Folder {
                                            path: folder.clone(),
                                            root: None,
                                        };
                                        Router::open(&r, destination, false, w, cx)
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("Rescan")
                                    .icon(IconName::RefreshCw)
                                    .disabled(scanning)
                                    .on_click(move |_, _, cx| {
                                        let _ = b.update(cx, |s, cx| {
                                            s.catalog.update(cx, |c, cx| c.rescan(id, cx))
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("Remove from Library…")
                                    .icon(IconName::X)
                                    .on_click(move |_, w, cx| {
                                        let _ =
                                            c.update(cx, |s, cx| s.confirm_remove_root(id, w, cx));
                                    }),
                            )
                            .separator();
                    Self::library_menu(menu)
                });
            rows.push(row.into_any_element());
        }
        let router = self.router.clone();
        rows.push(
            Self::opens(
                nav_row(
                    "liked".into(),
                    Icon::default().path(crate::assets::HEART_FILL),
                    "Liked Tracks".into(),
                    location == Some(Location::Liked),
                    &m,
                ),
                &self.router,
                Location::Liked,
            )
            .context_menu(move |menu, _, _| {
                Self::open_items(menu, router.clone(), Location::Liked, IconName::ListMusic)
            })
            .into_any_element(),
        );
        let router = self.router.clone();
        rows.push(
            Self::opens(
                nav_row(
                    "recently-played".into(),
                    Icon::new(IconName::Clock),
                    "Recently Played".into(),
                    location == Some(Location::RecentlyPlayed),
                    &m,
                ),
                &self.router,
                Location::RecentlyPlayed,
            )
            .context_menu(move |menu, _, _| {
                Self::open_items(
                    menu,
                    router.clone(),
                    Location::RecentlyPlayed,
                    IconName::Clock,
                )
            })
            .into_any_element(),
        );
        rows.extend(self.render_playlists(location, cx));
        rows
    }
    fn render_playlists(
        &self,
        location: Option<Location>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let m = *materials(cx);
        let playlists = self.catalog.read(cx).playlists.clone();
        let browse = self.router.clone();
        let header = h_flex()
            .id("playlists")
            .h(px(ROW))
            .mx(px(6.))
            .pl(px(8.))
            .pr(px(2.))
            .gap(px(4.))
            .flex_shrink_0()
            .items_center()
            .mt(px(6.))
            .text_size(px(12.))
            .text_color(m.text_muted)
            .child(
                div()
                    .id("browse-playlists")
                    .role(Role::Button)
                    .aria_label("Browse playlists")
                    .tab_index(0)
                    .cursor_pointer()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if location == Some(Location::Playlists) {
                        m.accent
                    } else {
                        m.text_muted
                    })
                    .hover(|d| d.text_color(m.text))
                    .on_click(move |_, w, cx| {
                        Self::open(&browse, Location::Playlists, false, w, cx)
                    })
                    .child("Playlists"),
            )
            .child(
                Button::new("new-playlist")
                    .ghost()
                    .xsmall()
                    .icon(
                        Icon::new(IconName::Plus)
                            .size(px(14.))
                            .text_color(m.text_faint),
                    )
                    .accessibility_label("New Playlist")
                    .tooltip("New Playlist · Ctrl+N")
                    .on_click(|_, w, cx| {
                        cx.stop_propagation();
                        w.dispatch_action(Box::new(crate::workspace::NewPlaylist), cx)
                    }),
            )
            .context_menu(|menu, _, _| {
                menu.item(
                    PopupMenuItem::new("New Playlist")
                        .icon(IconName::Plus)
                        .action(Box::new(crate::workspace::NewPlaylist)),
                )
            })
            .into_any_element();
        let mut rows = vec![header];
        for p in playlists {
            let id = p.id;
            let selected = location == Some(Location::Playlist(id));
            if let Some((_, input)) = self.renaming.as_ref().filter(|(r, _)| *r == id) {
                // The label gives way to the name field.
                rows.push(
                    h_flex()
                        .h(px(ROW))
                        .mx(px(6.))
                        .pl(px(8.))
                        .pr(px(8.))
                        .gap(px(4.))
                        .flex_shrink_0()
                        .items_center()
                        .key_context("LocationBar")
                        .on_action(cx.listener(|s, _: &crate::location::CancelEdit, w, cx| {
                            if s.renaming.take().is_some() {
                                s.focus.focus(w, cx);
                                cx.notify();
                            }
                        }))
                        .child(
                            Icon::new(IconName::Music2)
                                .size(px(16.))
                                .mr(px(2.))
                                .text_color(m.accent),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(input).xsmall().aria_label("Playlist name")),
                        )
                        .into_any_element(),
                );
                continue;
            }
            let row = nav_row(
                ("playlist", id as usize).into(),
                Icon::new(IconName::Music2),
                p.name.clone().into(),
                selected,
                &m,
            );
            let (player, catalog, router) = (
                self.player.clone(),
                self.catalog.clone(),
                self.router.clone(),
            );
            let this = cx.weak_entity();
            rows.push(
                playlist_target(
                    Self::opens(row, &self.router, Location::Playlist(id)),
                    id,
                    p.name.clone().into(),
                    catalog.clone(),
                    cx,
                )
                .context_menu(move |menu, _, _| {
                    let menu = Self::open_items(
                        menu,
                        router.clone(),
                        Location::Playlist(id),
                        IconName::ListMusic,
                    )
                    .separator();
                    playlist_menu(menu, id, catalog.clone(), player.clone(), this.clone())
                })
                .into_any_element(),
            );
        }
        rows
    }
}

/// Play, queue, rename, duplicate or delete a playlist.
fn playlist_menu(
    menu: PopupMenu,
    id: PlaylistId,
    catalog: Entity<Catalog>,
    player: Entity<Player>,
    this: WeakEntity<LibraryPanel>,
) -> PopupMenu {
    let play = move |next: Option<bool>, cx: &mut App| {
        let catalog_ = catalog.clone();
        let player = player.clone();
        let Some(store) = catalog_.read(cx).handle() else {
            return;
        };
        let Some(next) = next else {
            crate::table::play_library(
                &catalog_,
                &player,
                crate::catalog::ListeningList::Playlist(id),
                None,
                Command::Play,
                cx,
            );
            return;
        };
        let keys = store.call(move |s| {
            Ok(s.entries(id)?
                .into_iter()
                .map(|(e, _)| e)
                .collect::<Vec<_>>())
        });
        cx.spawn(async move |cx| {
            let Ok(keys) = keys.await else { return };
            cx.update(|cx| {
                let targets = Targets::Keys(Scope::Playlist(id), keys);
                let command = if next {
                    Command::PlayNext
                } else {
                    Command::Enqueue
                };
                queue_targets(&catalog_, &player, targets, command, cx);
            });
        })
        .detach();
    };
    let play = Arc::new(play);
    let (p1, p2, p3) = (play.clone(), play.clone(), play);
    let (a, b, c) = (this.clone(), this.clone(), this);
    menu.item(
        PopupMenuItem::new("Play")
            .icon(IconName::Play)
            .on_click(move |_, _, cx| p1(None, cx)),
    )
    .item(
        PopupMenuItem::new("Play Next")
            .icon(IconName::ListStart)
            .on_click(move |_, _, cx| p2(Some(true), cx)),
    )
    .item(
        PopupMenuItem::new("Add to Queue")
            .icon(IconName::ListEnd)
            .on_click(move |_, _, cx| p3(Some(false), cx)),
    )
    .separator()
    .item(
        PopupMenuItem::new("Rename")
            .icon(IconName::Pencil)
            .on_click(move |_, w, cx| {
                let _ = a.update(cx, |s, cx| s.rename_playlist(id, w, cx));
            }),
    )
    .item(
        PopupMenuItem::new("Duplicate")
            .icon(IconName::Copy)
            .on_click(move |_, _, cx| {
                let _ = b.update(cx, |s, cx| {
                    s.catalog.update(cx, |c, cx| c.duplicate_playlist(id, cx))
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("Delete Playlist…")
            .icon(IconName::Trash)
            .on_click(move |_, w, cx| {
                let _ = c.update(cx, |s, cx| s.confirm_delete_playlist(id, w, cx));
            }),
    )
}

/// Adds a folder to the library where it is.
fn add_folder(catalog: &Entity<Catalog>, path: PathBuf, cx: &mut App) {
    catalog.update(cx, |c, cx| c.add_root(path, cx));
}

impl Render for LibraryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let catalog = self.catalog.clone();
        v_flex()
            .id("library-panel")
            .group("library-panel")
            .key_context("Library")
            .track_focus(&self.focus)
            .size_full()
            .bg(m.chrome)
            // A folder from Locations, anywhere over the panel, joins the library.
            .drag_over::<DraggedFolder>(move |d, _, _, _| d.bg(m.accent.opacity(0.06)))
            .on_drop(move |d: &DraggedFolder, _, cx| add_folder(&catalog, d.path.clone(), cx))
            .context_menu(|menu, _, _| Self::library_menu(menu))
            .child(
                v_flex()
                    .id("library")
                    .flex_1()
                    .min_h_0()
                    .pt(px(4.))
                    .pb(px(6.))
                    .gap(px(1.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(self.render_rows(cx)),
            )
            .child(
                // Says what a folder dropped here will do, only while one is
                // dragged over the panel.
                h_flex()
                    .hidden()
                    .group_drag_over::<DraggedFolder>("library-panel", |s| s.flex())
                    .flex_shrink_0()
                    .h(px(34.))
                    .mx(px(8.))
                    .mb(px(8.))
                    .px(px(10.))
                    .gap(px(8.))
                    .items_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(m.accent.opacity(0.5))
                    .text_size(px(12.5))
                    .text_color(m.text)
                    .child(
                        Icon::new(IconName::FolderPlus)
                            .size(px(14.))
                            .text_color(m.accent),
                    )
                    .child("Add folder to Library"),
            )
    }
}

impl PaneContent for Entity<LibraryPanel> {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn view(&self) -> AnyView {
        self.clone().into()
    }
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus.clone()
    }
    fn title(&self, _: &App) -> SharedString {
        TYPE.title.into()
    }
    fn save(&self, cx: &App) -> serde_json::Value {
        serde_json::to_value(self.read(cx).state()).unwrap_or_default()
    }
    fn accepts_folder(&self) -> bool {
        true
    }
    fn drop_folder(&self, path: PathBuf, cx: &mut App) {
        let catalog = self.read(cx).catalog.clone();
        add_folder(&catalog, path, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::LibraryState;
    use crate::{catalog::Targets, store::Scope};

    #[test]
    fn the_old_playlist_disclosure_is_ignored_and_scroll_is_kept() {
        let state: LibraryState = serde_json::from_value(serde_json::json!({
            "playlists_closed": true, "scroll": 114.
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(state).unwrap(),
            serde_json::json!({"scroll": 114.})
        );
    }

    #[test]
    fn a_playlist_takes_tracks_and_other_playlists_entries_but_not_its_own() {
        assert!(Targets::Keys(Scope::Library(None), vec![1, 2]).can_copy_to(7));
        assert!(Targets::Keys(Scope::Playlist(3), vec![1, 1]).can_copy_to(7));
        assert!(Targets::Keys(Scope::Liked, vec![4]).can_copy_to(7));
        assert!(!Targets::Keys(Scope::Playlist(7), vec![1]).can_copy_to(7));
        assert!(!Targets::Tracks(vec![]).can_copy_to(7));
    }
}
