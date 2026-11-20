//! The navigation sidebar: Places, the library's collections, Liked Tracks
//! and playlists, then the folder tree.
use crate::{
    assets::FileIcon,
    audio::Command,
    browser::{Browser, Location},
    catalog::{Catalog, CatalogEvent, Targets},
    places::{Known, Origin, Place, Places, PlacesState, same_path},
    store::{PlaylistId, RootId, Scope},
    table::{DraggedTracks, queue_targets},
    theme::materials,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        Icon, Sizable, WindowExt,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputEvent, InputState},
        list::ListItem,
        menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
        tree::tree,
        v_flex,
    },
    prelude::*,
    *,
};
use std::{path::PathBuf, sync::Arc};

pub enum SidebarEvent {
    Changed,
}
impl EventEmitter<SidebarEvent> for Sidebar {}

#[derive(Clone)]
struct DraggedPlace {
    ix: usize,
    label: SharedString,
}
impl Render for DraggedPlace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = materials(cx);
        div()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(5.))
            .bg(m.overlay)
            .border_1()
            .border_color(m.control_line)
            .text_size(px(13.))
            .text_color(m.text)
            .child(self.label.clone())
    }
}

pub struct Sidebar {
    pub places: Places,
    browser: Entity<Browser>,
    catalog: Entity<Catalog>,
    renaming: Option<(usize, Entity<InputState>)>,
    renaming_playlist: Option<(PlaylistId, Entity<InputState>)>,
    available: Arc<[bool]>,
}

fn place_icon(p: &Place) -> Icon {
    match &p.origin {
        Origin::Known(Known::Music) => FileIcon::Music.icon(),
        Origin::Known(Known::Downloads) => Icon::new(IconName::Download),
        Origin::Known(_) => FileIcon::Folder.icon(),
        Origin::Drive => FileIcon::Drive.icon(),
        Origin::Custom => FileIcon::Folder.icon(),
    }
}

fn section(label: &'static str, cx: &App) -> Div {
    let m = materials(cx);
    h_flex()
        .h(px(30.))
        .px(px(16.))
        .flex_shrink_0()
        .items_center()
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(m.text_faint)
        .child(label)
}

/// A navigation row in the style of Places.
fn nav_row(
    id: ElementId,
    icon: Icon,
    label: SharedString,
    selected: bool,
    cx: &App,
) -> Stateful<Div> {
    let m = materials(cx);
    h_flex()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_stop(true)
        .h(px(28.))
        .mx(px(6.))
        .px(px(10.))
        .gap(px(10.))
        .rounded(px(5.))
        .items_center()
        .text_size(px(13.))
        .cursor_pointer()
        .text_color(if selected { m.text } else { m.text_muted })
        .when(selected, |d| d.bg(m.selected))
        .when(!selected, |d| d.hover(|d| d.bg(m.hover).text_color(m.text)))
        .child(icon.size(px(16.)).flex_shrink_0().text_color(if selected {
            m.accent
        } else {
            m.text_faint
        }))
        .child(div().flex_1().min_w_0().truncate().child(label))
}

fn divider(cx: &App) -> Div {
    div()
        .mx(px(16.))
        .my(px(2.))
        .h(px(1.))
        .flex_shrink_0()
        .bg(materials(cx).line)
}

impl Sidebar {
    pub fn new(
        browser: Entity<Browser>,
        catalog: Entity<Catalog>,
        saved: Option<PlacesState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&browser, |_, _, cx| cx.notify()).detach();
        cx.observe(&catalog, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(&catalog, window, |s, _, e, window, cx| {
            if let CatalogEvent::Created(id) = e {
                s.rename_playlist(*id, window, cx);
            }
        })
        .detach();
        let mut this = Self {
            places: Places::restore(saved),
            browser,
            catalog,
            renaming: None,
            renaming_playlist: None,
            available: Arc::from([]),
        };
        this.check_availability(cx);
        this.share_places(cx);
        this
    }
    fn show(&self, location: Location, window: &mut Window, cx: &mut App) {
        self.browser
            .update(cx, |b, cx| b.show(location, window, cx));
    }
    /// Starts editing a playlist's name in place.
    fn rename_playlist(&mut self, id: PlaylistId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.catalog.read(cx).playlist(id).map(|p| p.name.clone()) else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        cx.subscribe_in(&input, window, move |s, input, e, window, cx| match e {
            InputEvent::PressEnter { .. } | InputEvent::Blur
                if s.renaming_playlist.as_ref().is_some_and(|(p, _)| *p == id) =>
            {
                let name = input.read(cx).value().to_string();
                s.renaming_playlist = None;
                s.catalog
                    .update(cx, |c, cx| c.rename_playlist(id, name, cx));
                if matches!(e, InputEvent::PressEnter { .. }) {
                    s.focus_list(window, cx);
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
        self.renaming_playlist = Some((id, input));
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
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.check_availability(cx);
        self.share_places(cx);
        cx.emit(SidebarEvent::Changed);
        cx.notify();
    }
    /// Probes places off the interface thread; a sleeping network share must
    /// not stall the sidebar.
    fn check_availability(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.places.items.iter().map(|p| p.path.clone()).collect();
        cx.spawn(async move |this, cx| {
            let available: Arc<[bool]> = cx
                .background_spawn(async move { paths.iter().map(|p| p.is_dir()).collect() })
                .await;
            let _ = this.update(cx, |s, cx| {
                s.available = available;
                cx.notify();
            });
        })
        .detach();
    }
    pub fn add(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.places.add(path) {
            self.changed(cx);
        }
    }
    pub fn contains(&self, path: &std::path::Path) -> bool {
        self.places.contains(path)
    }
    fn open(&self, ix: usize, window: &mut Window, cx: &mut App) {
        if let Some(p) = self.places.items.get(ix) {
            let path = p.path.clone();
            self.browser
                .update(cx, |b, cx| b.open_place(path, window, cx));
        }
    }
    /// Places other than drives anchor the folder tree; a drive is where
    /// the tree falls back to anyway.
    fn share_places(&self, cx: &mut App) {
        let places = self
            .places
            .items
            .iter()
            .filter(|p| p.origin != Origin::Drive)
            .map(|p| p.path.clone())
            .collect();
        let folders = self.browser.read(cx).folders.clone();
        folders.update(cx, |f, _| f.set_places(places));
    }
    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add to Places".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = result.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |s, cx| s.add(path, cx));
            }
        })
        .detach();
    }
    fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(place) = self.places.items.get(ix) else {
            return;
        };
        let name = place.name();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        cx.subscribe_in(&input, window, move |s, input, e, window, cx| match e {
            InputEvent::PressEnter { .. } | InputEvent::Blur
                if s.renaming.as_ref().is_some_and(|(i, _)| *i == ix) =>
            {
                let label = input.read(cx).value().to_string();
                s.renaming = None;
                s.places.rename(ix, &label);
                s.changed(cx);
                if matches!(e, InputEvent::PressEnter { .. }) {
                    s.focus_list(window, cx);
                }
            }
            _ => {}
        })
        .detach();
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.renaming = Some((ix, input));
        cx.notify();
    }
    fn focus_list(&self, window: &mut Window, cx: &mut App) {
        let table = self.browser.read(cx).table.clone();
        table.read(cx).focus_handle(cx).focus(window, cx);
    }
    fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            self.focus_list(window, cx);
            cx.notify();
        }
    }
    fn confirm_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title("Reset Places to defaults?")
                .description(
                    "Your shortcuts, names and order will be replaced by Music, Downloads and \
                     this computer's drives. Folders on disk are not changed.",
                )
                .show_cancel(true)
                .ok_text("Reset Places")
                .on_ok(move |_, _, cx| {
                    let _ = this.update(cx, |s, cx| {
                        s.renaming = None;
                        s.places.reset();
                        s.changed(cx);
                    });
                    true
                })
        });
    }
    fn section_menu(this: WeakEntity<Self>, menu: PopupMenu, missing: usize) -> PopupMenu {
        let (a, b, c) = (this.clone(), this.clone(), this);
        menu.item(
            PopupMenuItem::new("Add Folder to Places…")
                .icon(IconName::FolderPlus)
                .on_click(move |_, w, cx| {
                    let _ = a.update(cx, |s, cx| s.choose_folder(w, cx));
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new(if missing > 0 {
                format!("Restore Missing Default Places ({missing})")
            } else {
                "Restore Missing Default Places".into()
            })
            .disabled(missing == 0)
            .on_click(move |_, _, cx| {
                let _ = b.update(cx, |s, cx| {
                    s.places.restore_missing();
                    s.changed(cx);
                });
            }),
        )
        .item(
            PopupMenuItem::new("Reset Places to Defaults…").on_click(move |_, w, cx| {
                let _ = c.update(cx, |s, cx| s.confirm_reset(w, cx));
            }),
        )
    }
    fn render_place(
        &self,
        ix: usize,
        current: Option<&std::path::Path>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let m = *materials(cx);
        let place = &self.places.items[ix];
        let label: SharedString = place.name().into();
        let selected = current.is_some_and(|c| same_path(&place.path, c));
        let catalog = self.catalog.clone();
        let place_path = place.path.clone();
        let available = self.available.get(ix).copied().unwrap_or(true);
        let this = cx.weak_entity();
        let renaming = self.renaming.as_ref().filter(|(i, _)| *i == ix);
        let row = h_flex()
            .id(("place", ix))
            .h(px(28.))
            .mx(px(6.))
            .px(px(10.))
            .gap(px(10.))
            .rounded(px(5.))
            .items_center()
            .text_size(px(13.))
            .text_color(if selected { m.text } else { m.text_muted })
            .when(selected, |d| d.bg(m.selected))
            .when(!selected, |d| d.hover(|d| d.bg(m.hover).text_color(m.text)))
            .when(!available, |d| d.opacity(0.45))
            .child(
                place_icon(place)
                    .size(px(16.))
                    .flex_shrink_0()
                    .text_color(if selected { m.accent } else { m.text_faint }),
            );
        if let Some((_, input)) = renaming {
            return row
                .key_context("LocationBar")
                .on_action(
                    cx.listener(|s, _: &crate::location::CancelEdit, w, cx| s.cancel_rename(w, cx)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(input).xsmall().aria_label("Shortcut name")),
                )
                .into_any_element();
        }
        // Only an unavailable place explains itself; elsewhere a tooltip would
        // compete with the context menu and the address bar already shows the path.
        let unavailable =
            (!available).then(|| format!("{} is not available", place.path.display()));
        row.child(div().flex_1().min_w_0().truncate().child(label.clone()))
            .cursor_pointer()
            .when_some(unavailable, |d, tip| {
                d.tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                })
            })
            .on_click(cx.listener(move |s, _, w, cx| s.open(ix, w, cx)))
            .on_drag(
                DraggedPlace {
                    ix,
                    label: label.clone(),
                },
                |d, _, _, cx| cx.new(|_| d.clone()),
            )
            .drag_over::<DraggedPlace>(move |d, _, _, _| d.border_t_2().border_color(m.accent))
            .on_drop(cx.listener(move |s, d: &DraggedPlace, _, cx| {
                s.places.move_item(d.ix, ix);
                s.changed(cx);
            }))
            .context_menu(move |menu, _, _| {
                let (a, b, c) = (this.clone(), this.clone(), this.clone());
                let (catalog, path) = (catalog.clone(), place_path.clone());
                menu.item(
                    PopupMenuItem::new("Open")
                        .icon(IconName::FolderOpen)
                        .on_click(move |_, w, cx| {
                            let _ = a.update(cx, |s, cx| s.open(ix, w, cx));
                        }),
                )
                .item(
                    PopupMenuItem::new("Add to Library")
                        .icon(IconName::Library)
                        .on_click(move |_, _, cx| {
                            catalog.update(cx, |c, cx| c.add_root(path.clone(), cx))
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Rename Shortcut")
                        .icon(IconName::Pencil)
                        .on_click(move |_, w, cx| {
                            let _ = b.update(cx, |s, cx| s.start_rename(ix, w, cx));
                        }),
                )
                .item(
                    PopupMenuItem::new("Remove from Places")
                        .icon(IconName::X)
                        .on_click(move |_, _, cx| {
                            let _ = c.update(cx, |s, cx| {
                                s.renaming = None;
                                s.places.remove(ix);
                                s.changed(cx);
                            });
                        }),
                )
            })
            .into_any_element()
    }
    /// The folder tree. The expander only opens or closes a branch; the
    /// name opens the folder. Both resolve their entry by path, so a click
    /// that lands while the tree updates still means the folder it showed.
    fn render_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let b = self.browser.read(cx);
        let state = b.folders.read(cx).state.clone();
        let folders = b.folders.clone();
        let browser = self.browser.clone();
        let menu_folders = b.folders.clone();
        let menu_browser = self.browser.clone();
        let sidebar = cx.weak_entity();
        let catalog = self.catalog.clone();
        tree(&state, move |ix, entry, selected, _, _| {
            let depth = entry.depth();
            let id = entry.item().id.clone();
            let toggle_id = id.clone();
            let (browser, folders, toggle) = (browser.clone(), folders.clone(), folders.clone());
            let expandable = entry.is_folder();
            ListItem::new(("folder", ix))
                .h(px(26.))
                .pl(px(4.))
                .pr(px(10.))
                .py_0()
                .mx(px(6.))
                .rounded(px(5.))
                .child(
                    h_flex()
                        .gap(px(4.))
                        .min_w_0()
                        .items_center()
                        .text_size(px(13.))
                        .text_color(if selected { m.text } else { m.text_muted })
                        .pl(px(depth as f32 * 14.))
                        .child(
                            div()
                                .id(("folder-expander", ix))
                                .size(px(20.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .when(expandable, |d| {
                                    d.hover(|d| d.bg(m.hover))
                                        .child(
                                            Icon::new(if entry.is_expanded() {
                                                IconName::ChevronDown
                                            } else {
                                                IconName::ChevronRight
                                            })
                                            .size(px(12.))
                                            .text_color(m.text_faint),
                                        )
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            toggle.update(cx, |f, cx| f.toggle(&toggle_id, cx));
                                        })
                                }),
                        )
                        .child(
                            FileIcon::Folder
                                .icon()
                                .size(px(15.))
                                .flex_shrink_0()
                                .mr(px(2.))
                                .text_color(if selected { m.accent } else { m.text_faint }),
                        )
                        .child(div().truncate().child(entry.item().label.clone())),
                )
                .on_click(move |_, window, cx| {
                    // The tree's own click handling would select by row
                    // position and expand; navigation does both by path.
                    cx.stop_propagation();
                    let b = browser.read(cx);
                    if let Some(path) = folders.read(cx).path(&id)
                        && (!same_path(&path, &b.path) || !b.location.is_folder())
                    {
                        browser.update(cx, |b, cx| b.open(path, window, cx));
                    }
                })
        })
        .context_menu(move |_, entry, menu, _, cx| {
            let Some(path) = menu_folders.read(cx).path(&entry.item().id) else {
                return menu;
            };
            let pinned = sidebar
                .upgrade()
                .is_some_and(|s| s.read(cx).contains(&path));
            let (b, p, s) = (menu_browser.clone(), path.clone(), sidebar.clone());
            let (catalog, root) = (catalog.clone(), path.clone());
            menu.item(
                PopupMenuItem::new("Open")
                    .icon(IconName::FolderOpen)
                    .on_click(move |_, w, cx| b.update(cx, |b, cx| b.open(p.clone(), w, cx))),
            )
            .item(
                PopupMenuItem::new(if pinned {
                    "Already in Places"
                } else {
                    "Add to Places"
                })
                .icon(IconName::Pin)
                .disabled(pinned)
                .on_click(move |_, _, cx| {
                    let _ = s.update(cx, |s, cx| s.add(path.clone(), cx));
                }),
            )
            .item(
                PopupMenuItem::new("Add to Library")
                    .icon(IconName::Library)
                    .on_click(move |_, _, cx| {
                        catalog.update(cx, |c, cx| c.add_root(root.clone(), cx))
                    }),
            )
        })
    }
}

impl Sidebar {
    /// Liked Tracks and the registered collections.
    fn render_library(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let location = self.browser.read(cx).location.clone();
        let catalog = self.catalog.read(cx);
        let mut rows = vec![
            nav_row(
                "liked".into(),
                Icon::default().path(crate::assets::HEART_FILL),
                "Liked Tracks".into(),
                location == Location::Liked,
                cx,
            )
            .on_click(cx.listener(|s, _, w, cx| s.show(Location::Liked, w, cx)))
            .into_any_element(),
        ];
        if catalog.collections.len() > 1 {
            rows.push(
                nav_row(
                    "all-music".into(),
                    Icon::new(IconName::Library),
                    "All Music".into(),
                    location == Location::Library(None),
                    cx,
                )
                .on_click(cx.listener(|s, _, w, cx| s.show(Location::Library(None), w, cx)))
                .into_any_element(),
            );
        }
        for c in &catalog.collections {
            let id = c.root.id;
            let path = c.root.path.clone();
            let status = c.status.clone();
            let tip = (!c.online).then(|| format!("{} is not available", path.display()));
            let this = cx.weak_entity();
            let browser = self.browser.clone();
            let scanning = catalog.is_scanning(id);
            let row = nav_row(
                ("collection", id as usize).into(),
                FileIcon::Music.icon(),
                c.root.name.clone().into(),
                location == Location::Library(Some(id))
                    || matches!(location, Location::Album {root:Some(r),..} if r == id),
                cx,
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
                d.tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                })
            })
            .on_click(cx.listener(move |s, _, w, cx| s.show(Location::Library(Some(id)), w, cx)))
            .context_menu(move |menu, _, _| {
                let (a, b, c) = (this.clone(), this.clone(), browser.clone());
                let path = path.clone();
                menu.item(PopupMenuItem::new("Open").icon(IconName::Library).on_click(
                    move |_, w, cx| {
                        let _ = a.update(cx, |s, cx| s.show(Location::Library(Some(id)), w, cx));
                    },
                ))
                .item(
                    PopupMenuItem::new("Browse Folder")
                        .icon(IconName::FolderOpen)
                        .on_click(move |_, w, cx| {
                            c.update(cx, |b, cx| b.open_place(path.clone(), w, cx))
                        }),
                )
                .item({
                    let this = b.clone();
                    PopupMenuItem::new("Rescan")
                        .icon(IconName::RefreshCw)
                        .disabled(scanning)
                        .on_click(move |_, _, cx| {
                            let _ = this
                                .update(cx, |s, cx| s.catalog.update(cx, |c, cx| c.rescan(id, cx)));
                        })
                })
                .separator()
                .item(
                    PopupMenuItem::new("Remove from Library…")
                        .icon(IconName::X)
                        .on_click(move |_, w, cx| {
                            let _ = b.update(cx, |s, cx| s.confirm_remove_root(id, w, cx));
                        }),
                )
            })
            .into_any_element();
            rows.push(row);
        }
        if catalog.loaded && catalog.collections.is_empty() {
            rows.push(
                nav_row(
                    "add-collection".into(),
                    Icon::new(IconName::FolderPlus),
                    "Add Music Folder…".into(),
                    false,
                    cx,
                )
                .text_color(m.text_faint)
                .on_click(|_, w, cx| {
                    w.dispatch_action(Box::new(crate::workspace::AddMusicFolder), cx)
                })
                .into_any_element(),
            );
        }
        v_flex()
            .id("library")
            .flex_shrink_0()
            .pb(px(6.))
            .child(section("Library", cx))
            .child(v_flex().gap(px(1.)).children(rows))
            .context_menu(|menu, _, _| {
                menu.item(
                    PopupMenuItem::new("Add Music Folder…")
                        .icon(IconName::FolderPlus)
                        .action(Box::new(crate::workspace::AddMusicFolder)),
                )
            })
    }
    fn render_playlists(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let location = self.browser.read(cx).location.clone();
        let playlists = self.catalog.read(cx).playlists.clone();
        let mut rows = vec![];
        for p in playlists {
            let id = p.id;
            let selected = location == Location::Playlist(id);
            let row = nav_row(
                ("playlist", id as usize).into(),
                Icon::new(IconName::ListMusic),
                p.name.clone().into(),
                selected,
                cx,
            );
            if let Some((_, input)) = self.renaming_playlist.as_ref().filter(|(r, _)| *r == id) {
                // The label gives way to the name field.
                rows.push(
                    h_flex()
                        .h(px(28.))
                        .mx(px(6.))
                        .px(px(10.))
                        .gap(px(10.))
                        .items_center()
                        .key_context("LocationBar")
                        .on_action(cx.listener(|s, _: &crate::location::CancelEdit, w, cx| {
                            if s.renaming_playlist.take().is_some() {
                                s.focus_list(w, cx);
                                cx.notify();
                            }
                        }))
                        .child(
                            Icon::new(IconName::ListMusic)
                                .size(px(16.))
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
            let (player, catalog) = (
                self.browser
                    .read(cx)
                    .table
                    .read(cx)
                    .delegate()
                    .player
                    .clone(),
                self.catalog.clone(),
            );
            let this = cx.weak_entity();
            rows.push(
                row.on_click(cx.listener(move |s, _, w, cx| s.show(Location::Playlist(id), w, cx)))
                    .drag_over::<DraggedTracks>(move |s, _, _, _| {
                        s.bg(m.selected)
                            .border_1()
                            .border_color(m.accent.opacity(0.6))
                    })
                    .on_drop(cx.listener(move |s, d: &DraggedTracks, _, cx| {
                        // Dropping a playlist's own entries onto it adds copies,
                        // which is rarely meant; reordering happens in the table.
                        if !matches!(d.targets.as_ref(), Targets::Keys(Scope::Playlist(p), _) if p == &id) {
                            let targets = d.targets.as_ref().clone();
                            s.catalog
                                .update(cx, |c, cx| c.add_to_playlist(id, targets, cx));
                        }
                    }))
                    .context_menu(move |menu, _, _| {
                        let play = {
                            let (catalog, player) = (catalog.clone(), player.clone());
                            move |next: Option<bool>, cx: &mut App| {
                                let catalog_ = catalog.clone();
                                let player = player.clone();
                                let Some(store) = catalog_.read(cx).handle() else {
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
                                        match next {
                                            None => queue_targets(
                                                &catalog_,
                                                &player,
                                                targets,
                                                |t| Command::Play(t, 0),
                                                cx,
                                            ),
                                            Some(true) => queue_targets(
                                                &catalog_,
                                                &player,
                                                targets,
                                                Command::PlayNext,
                                                cx,
                                            ),
                                            Some(false) => queue_targets(
                                                &catalog_,
                                                &player,
                                                targets,
                                                Command::Enqueue,
                                                cx,
                                            ),
                                        }
                                    });
                                })
                                .detach();
                            }
                        };
                        let (p1, p2, p3) = (play.clone(), play.clone(), play);
                        let (a, b, c) = (this.clone(), this.clone(), this.clone());
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
                                    let _ =
                                        c.update(cx, |s, cx| s.confirm_delete_playlist(id, w, cx));
                                }),
                        )
                    })
                    .into_any_element(),
            );
        }
        let header = section("Playlists", cx)
            .pr(px(8.))
            .child(div().flex_1())
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
                        w.dispatch_action(Box::new(crate::workspace::NewPlaylist), cx)
                    }),
            );
        v_flex()
            .id("playlists")
            .flex_shrink_0()
            .pb(px(6.))
            .child(header)
            .child(v_flex().gap(px(1.)).children(rows))
            .context_menu(|menu, _, _| {
                menu.item(
                    PopupMenuItem::new("New Playlist")
                        .icon(IconName::Plus)
                        .action(Box::new(crate::workspace::NewPlaylist)),
                )
            })
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let b = self.browser.read(cx);
        let current = b.location.is_folder().then(|| b.path.clone());
        let missing = self.places.missing_defaults();
        let this = cx.weak_entity();
        let rows = (0..self.places.items.len())
            .map(|ix| self.render_place(ix, current.as_deref(), cx))
            .collect::<Vec<_>>();
        let end_drop = cx.listener(|s, d: &DraggedPlace, _, cx| {
            let end = s.places.items.len();
            s.places.move_item(d.ix, end);
            s.changed(cx);
        });
        let places = v_flex()
            .id("places")
            .flex_shrink_0()
            .pb(px(6.))
            .child(section("Places", cx))
            .child(v_flex().gap(px(1.)).children(rows))
            // A strip below the last place takes drops to the end.
            .child(
                div()
                    .id("places-end")
                    .h(px(6.))
                    .mx(px(6.))
                    .drag_over::<DraggedPlace>(move |d, _, _, _| {
                        d.border_t_2().border_color(m.accent)
                    })
                    .on_drop(end_drop),
            )
            .context_menu(move |menu, _, _| Self::section_menu(this.clone(), menu, missing));
        v_flex()
            .size_full()
            .bg(m.chrome)
            .pt(px(2.))
            // Shortcuts, the library and playlists scroll together and leave
            // the folder tree at least a third of the sidebar.
            .child(
                v_flex()
                    .id("navigation")
                    .flex_shrink_0()
                    .max_h(relative(0.66))
                    .overflow_y_scroll()
                    .child(places)
                    .child(divider(cx))
                    .child(self.render_library(cx))
                    .child(divider(cx))
                    .child(self.render_playlists(cx)),
            )
            .child(divider(cx))
            .child(section("Folders", cx).mt(px(4.)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .pb(px(6.))
                    .child(self.render_tree(cx)),
            )
    }
}
