//! The navigation sidebar, in two parts.
//!
//! **Library** is personal music organisation: the indexed collections,
//! Liked Tracks and playlists. **Locations** is the file system: every Place
//! is the root of its own folder hierarchy and expands in place, so the
//! folders of Music appear under Music rather than in a second tree.
//!
//! A registered collection and a Place may point at the same folder; the
//! first opens the index, the second the folder on disk.
use crate::{
    assets::FileIcon,
    audio::Command,
    browser::{Browser, Location},
    catalog::{Catalog, CatalogEvent, Targets},
    folders::{ExpansionChanged, Folders},
    places::{Known, Origin, Places, PlacesState, same_path},
    store::{PlaylistId, RootId, Scope},
    table::{DraggedTracks, queue_targets},
    theme::{Materials, materials},
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
        tooltip::Tooltip,
        tree::tree,
        v_flex,
    },
    prelude::*,
    *,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

const ROW: f32 = 26.;
/// Each level of a hierarchy, in Library and Locations alike.
const INDENT: f32 = 14.;

/// What the sidebar remembers between sessions besides the Places.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarState {
    /// Folders opened in Locations, by root and path.
    pub expanded: Vec<String>,
    pub playlists_closed: bool,
}

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
    folders: Entity<Folders>,
    renaming: Option<(usize, Entity<InputState>)>,
    renaming_playlist: Option<(PlaylistId, Entity<InputState>)>,
    available: Arc<[bool]>,
    playlists_open: bool,
}

fn place_icon(origin: &Origin) -> Icon {
    match origin {
        Origin::Known(Known::Music) => FileIcon::Music.icon(),
        Origin::Known(Known::Downloads) => Icon::new(IconName::Download),
        Origin::Known(_) => FileIcon::Folder.icon(),
        Origin::Drive => FileIcon::Drive.icon(),
        Origin::Custom => FileIcon::Folder.icon(),
    }
}

/// A restrained heading over a part of the sidebar.
fn section(id: &'static str, label: &'static str, cx: &App) -> Stateful<Div> {
    let m = materials(cx);
    h_flex()
        .id(id)
        .h(px(30.))
        .pl(px(16.))
        .pr(px(8.))
        .flex_shrink_0()
        .items_center()
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(m.text_faint)
        .child(div().flex_1().child(label))
}

/// The disclosure slot that starts every row, so icons line up whether or
/// not the row opens.
fn disclosure(id: impl Into<ElementId>, open: Option<bool>, m: &Materials) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(20.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .when_some(open, |d, open| {
            d.hover(|d| d.bg(m.hover)).child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(px(12.))
                .text_color(m.text_faint),
            )
        })
}

/// A Library row, laid out as a Locations row: disclosure slot, icon, label.
fn nav_row(
    id: ElementId,
    depth: usize,
    icon: Option<Icon>,
    label: SharedString,
    selected: bool,
    m: &Materials,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_stop(true)
        .h(px(ROW))
        .mx(px(6.))
        .pl(px(4. + depth as f32 * INDENT))
        .pr(px(8.))
        .gap(px(4.))
        .rounded(px(5.))
        .flex_shrink_0()
        .items_center()
        .text_size(px(13.))
        .cursor_pointer()
        .text_color(if selected { m.text } else { m.text_muted })
        .when(selected, |d| d.bg(m.selected))
        .when(!selected, |d| d.hover(|d| d.bg(m.hover).text_color(m.text)))
        .child(div().size(px(20.)).flex_shrink_0())
        .when_some(icon, |d, icon| {
            d.child(
                icon.size(px(16.))
                    .flex_shrink_0()
                    .mr(px(2.))
                    .text_color(if selected { m.accent } else { m.text_faint }),
            )
        })
        .child(div().flex_1().min_w_0().truncate().child(label))
}

fn divider(cx: &App) -> Div {
    div()
        .mx(px(16.))
        .my(px(4.))
        .h(px(1.))
        .flex_shrink_0()
        .bg(materials(cx).line)
}

/// What a Locations row needs to know about the Places, captured when the
/// sidebar renders.
#[derive(Clone)]
struct PlaceRow {
    path: PathBuf,
    name: SharedString,
    origin: Origin,
    available: bool,
}

impl Sidebar {
    pub fn new(
        browser: Entity<Browser>,
        catalog: Entity<Catalog>,
        saved: Option<PlacesState>,
        state: SidebarState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&browser, |_, _, cx| cx.notify()).detach();
        cx.observe(&catalog, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(&catalog, window, |s, _, e, window, cx| {
            if let CatalogEvent::Created(id) = e {
                s.playlists_open = true;
                s.rename_playlist(*id, window, cx);
            }
        })
        .detach();
        let folders = browser.read(cx).folders.clone();
        folders.update(cx, |f, _| f.restore_expanded(state.expanded));
        cx.subscribe(&folders, |_, _, _: &ExpansionChanged, cx| {
            cx.emit(SidebarEvent::Changed)
        })
        .detach();
        let mut this = Self {
            places: Places::restore(saved),
            browser,
            catalog,
            folders,
            renaming: None,
            renaming_playlist: None,
            available: Arc::from([]),
            playlists_open: !state.playlists_closed,
        };
        this.check_availability(cx);
        this.share_places(cx);
        this
    }
    pub fn state(&self, cx: &App) -> SidebarState {
        SidebarState {
            expanded: self.folders.read(cx).expanded_state(),
            playlists_closed: !self.playlists_open,
        }
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
    /// Every Place is a root of Locations; drives yield to the other
    /// Places when choosing where to show a folder.
    fn share_places(&self, cx: &mut App) {
        let places = self
            .places
            .items
            .iter()
            .map(|p| (p.path.clone(), p.origin == Origin::Drive))
            .collect();
        self.folders.update(cx, |f, cx| f.set_places(places, cx));
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
    fn locations_menu(this: WeakEntity<Self>, menu: PopupMenu, missing: usize) -> PopupMenu {
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
    fn place_menu(
        this: WeakEntity<Self>,
        catalog: Entity<Catalog>,
        browser: Entity<Browser>,
        ix: usize,
        path: PathBuf,
        menu: PopupMenu,
    ) -> PopupMenu {
        let (b, c) = (this.clone(), this);
        let open = path.clone();
        menu.item(
            PopupMenuItem::new("Open")
                .icon(IconName::FolderOpen)
                .on_click(move |_, w, cx| {
                    browser.update(cx, |b, cx| b.open_place(open.clone(), w, cx))
                }),
        )
        .item(
            PopupMenuItem::new("Add to Library")
                .icon(IconName::Library)
                .on_click(move |_, _, cx| catalog.update(cx, |c, cx| c.add_root(path.clone(), cx))),
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
    }
    fn folder_menu(
        sidebar: WeakEntity<Self>,
        catalog: Entity<Catalog>,
        browser: Entity<Browser>,
        root: PathBuf,
        path: PathBuf,
        pinned: bool,
        menu: PopupMenu,
    ) -> PopupMenu {
        let (open, add, library) = (path.clone(), path.clone(), path);
        menu.item(
            PopupMenuItem::new("Open")
                .icon(IconName::FolderOpen)
                .on_click(move |_, w, cx| {
                    browser.update(cx, |b, cx| b.open_in(open.clone(), root.clone(), w, cx))
                }),
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
                let _ = sidebar.update(cx, |s, cx| s.add(add.clone(), cx));
            }),
        )
        .item(
            PopupMenuItem::new("Add to Library")
                .icon(IconName::Library)
                .on_click(move |_, _, cx| {
                    catalog.update(cx, |c, cx| c.add_root(library.clone(), cx))
                }),
        )
    }

    /// Locations: one tree whose roots are the Places. The disclosure only
    /// opens or closes a branch; the name opens the folder. Both resolve
    /// their entry by root and path, so a click that lands while the tree
    /// updates still means the folder it showed.
    fn render_locations(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let state = self.folders.read(cx).state.clone();
        let places: Arc<[PlaceRow]> = self
            .places
            .items
            .iter()
            .enumerate()
            .map(|(i, p)| PlaceRow {
                path: p.path.clone(),
                name: p.name().into(),
                origin: p.origin.clone(),
                available: self.available.get(i).copied().unwrap_or(true),
            })
            .collect();
        let renaming = self.renaming.clone();
        let (folders, browser, sidebar) =
            (self.folders.clone(), self.browser.clone(), cx.weak_entity());
        let menu_places = places.clone();
        let (menu_folders, menu_browser, menu_sidebar, catalog) = (
            self.folders.clone(),
            self.browser.clone(),
            cx.weak_entity(),
            self.catalog.clone(),
        );
        tree(&state, move |ix, entry, selected, _, cx| {
            let depth = entry.depth();
            let id = entry.item().id.clone();
            let Some((root, path)) = folders.read(cx).entry(&id) else {
                return ListItem::new(("location", ix)).h(px(ROW));
            };
            let place = (depth == 0)
                .then(|| places.iter().position(|p| same_path(&p.path, &root)))
                .flatten();
            let temporary = depth == 0 && place.is_none();
            let available = place.is_none_or(|i| places[i].available);
            let label: SharedString = match place {
                Some(i) => places[i].name.clone(),
                None if temporary => crate::library::display_name(&root).into(),
                None => entry.item().label.clone(),
            };
            let icon = match place {
                Some(i) => place_icon(&places[i].origin),
                None if temporary => FileIcon::Drive.icon(),
                None => FileIcon::Folder.icon(),
            };
            let toggle = folders.clone();
            let toggle_id = id.clone();
            let expander = disclosure(
                ("location-disclosure", ix),
                entry.is_folder().then(|| entry.is_expanded()),
                &m,
            )
            .when(entry.is_folder(), |d| {
                d.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        toggle.update(cx, |f, cx| f.toggle(&toggle_id, cx));
                    })
            });
            let content = h_flex()
                .id(("location-row", ix))
                .size_full()
                .gap(px(4.))
                .min_w_0()
                .items_center()
                .text_size(px(13.))
                .text_color(if selected {
                    m.text
                } else if temporary {
                    m.text_faint
                } else {
                    m.text_muted
                })
                .pl(px(depth as f32 * INDENT))
                .when(!available, |d| d.opacity(0.45))
                .child(expander)
                .child(
                    icon.size(px(if depth == 0 { 16. } else { 15. }))
                        .flex_shrink_0()
                        .mr(px(2.))
                        .text_color(if selected { m.accent } else { m.text_faint }),
                );
            let editing = renaming
                .as_ref()
                .filter(|(i, _)| Some(*i) == place)
                .map(|(_, input)| input.clone());
            let content = if let Some(input) = editing {
                let s = sidebar.clone();
                content
                    .key_context("LocationBar")
                    // Clicks inside the name field neither open nor fold the row.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .on_action(move |_: &crate::location::CancelEdit, w, cx| {
                        let _ = s.update(cx, |s, cx| s.cancel_rename(w, cx));
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&input).xsmall().aria_label("Shortcut name")),
                    )
            } else {
                let (b, f) = (browser.clone(), folders.clone());
                let open = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    // The tree's own click handling would select by row
                    // position and expand; navigation does both by path.
                    cx.stop_propagation();
                    let Some((root, path)) = f.read(cx).entry(&id) else {
                        return;
                    };
                    let shown = f
                        .read(cx)
                        .current()
                        .is_some_and(|(r, c)| same_path(r, &root) && same_path(c, &path));
                    if !shown || !b.read(cx).location.is_folder() {
                        b.update(cx, |b, cx| b.open_in(path, root, window, cx));
                    }
                };
                content
                    .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                    .on_click(open)
                    .when(!available, |d| {
                        let tip = format!("{} is not available", path.display());
                        d.tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                    })
                    .when(temporary, |d| {
                        let tip = format!("{} is not in Places", root.display());
                        d.tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                    })
                    .when_some(place, |d, i| {
                        let s = sidebar.clone();
                        d.on_drag(DraggedPlace { ix: i, label }, |d, _, _, cx| {
                            cx.new(|_| d.clone())
                        })
                        .drag_over::<DraggedPlace>(move |d, _, _, _| {
                            d.border_t_2().border_color(m.accent)
                        })
                        .on_drop(move |d: &DraggedPlace, _, cx| {
                            let _ = s.update(cx, |s, cx| {
                                s.places.move_item(d.ix, i);
                                s.changed(cx);
                            });
                        })
                    })
            };
            ListItem::new(("location", ix))
                .h(px(ROW))
                .pl(px(4.))
                .pr(px(8.))
                .py_0()
                .mx(px(6.))
                .rounded(px(5.))
                .child(content)
        })
        .context_menu(move |_, entry, menu, _, cx| {
            let Some((root, path)) = menu_folders.read(cx).entry(&entry.item().id) else {
                return menu;
            };
            if entry.depth() == 0
                && let Some(ix) = menu_places.iter().position(|p| same_path(&p.path, &root))
            {
                return Self::place_menu(
                    menu_sidebar.clone(),
                    catalog.clone(),
                    menu_browser.clone(),
                    ix,
                    path,
                    menu,
                );
            }
            let pinned = menu_places.iter().any(|p| same_path(&p.path, &path));
            Self::folder_menu(
                menu_sidebar.clone(),
                catalog.clone(),
                menu_browser.clone(),
                root,
                path,
                pinned,
                menu,
            )
        })
    }
}

impl Sidebar {
    /// The collections, Liked Tracks and the playlists.
    fn render_library(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let m = *materials(cx);
        let location = self.browser.read(cx).location.clone();
        let catalog = self.catalog.read(cx);
        let mut rows = vec![];
        let single = catalog.collections.len() == 1;
        // With one collection, its own row stands for All Music.
        if !single {
            rows.push(
                nav_row(
                    "all-music".into(),
                    0,
                    Some(Icon::new(IconName::Library)),
                    "All Music".into(),
                    matches!(
                        location,
                        Location::Library(None)
                            | Location::Album { root: None, .. }
                            | Location::Release { root: None, .. }
                    ),
                    &m,
                )
                .on_click(cx.listener(|s, _, w, cx| s.show(Location::Library(None), w, cx)))
                .context_menu(|menu, _, _| Self::library_menu(menu))
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
            let selected = match location {
                Location::Library(Some(r))
                | Location::Album { root: Some(r), .. }
                | Location::Release { root: Some(r), .. } => r == id,
                Location::Library(None) => single,
                _ => false,
            };
            let row = nav_row(
                ("collection", id as usize).into(),
                usize::from(!single),
                Some(Icon::new(if single {
                    IconName::Library
                } else {
                    IconName::Disc3
                })),
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
            })
            .on_click(cx.listener(move |s, _, w, cx| s.show(Location::Library(Some(id)), w, cx)))
            .context_menu(move |menu, _, _| {
                let (a, b, c) = (this.clone(), this.clone(), browser.clone());
                let path = path.clone();
                let menu = menu
                    .item(PopupMenuItem::new("Open").icon(IconName::Library).on_click(
                        move |_, w, cx| {
                            let _ =
                                a.update(cx, |s, cx| s.show(Location::Library(Some(id)), w, cx));
                        },
                    ))
                    .item(
                        PopupMenuItem::new("Browse Folder")
                            .icon(IconName::FolderOpen)
                            .on_click(move |_, w, cx| {
                                c.update(cx, |b, cx| b.open(path.clone(), w, cx))
                            }),
                    )
                    .item({
                        let this = b.clone();
                        PopupMenuItem::new("Rescan")
                            .icon(IconName::RefreshCw)
                            .disabled(scanning)
                            .on_click(move |_, _, cx| {
                                let _ = this.update(cx, |s, cx| {
                                    s.catalog.update(cx, |c, cx| c.rescan(id, cx))
                                });
                            })
                    })
                    .item(
                        PopupMenuItem::new("Remove from Library…")
                            .icon(IconName::X)
                            .on_click(move |_, w, cx| {
                                let _ = b.update(cx, |s, cx| s.confirm_remove_root(id, w, cx));
                            }),
                    )
                    .separator();
                Self::library_menu(menu)
            })
            .into_any_element();
            rows.push(row);
        }
        rows.push(
            nav_row(
                "liked".into(),
                0,
                Some(Icon::default().path(crate::assets::HEART_FILL)),
                "Liked Tracks".into(),
                location == Location::Liked,
                &m,
            )
            .on_click(cx.listener(|s, _, w, cx| s.show(Location::Liked, w, cx)))
            .into_any_element(),
        );
        rows.extend(self.render_playlists(cx));
        rows
    }
    fn render_playlists(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let m = *materials(cx);
        let location = self.browser.read(cx).location.clone();
        let playlists = self.catalog.read(cx).playlists.clone();
        let open = self.playlists_open;
        let header = h_flex()
            .id("playlists")
            .group("playlists")
            .h(px(ROW))
            .mx(px(6.))
            .pl(px(4.))
            .pr(px(2.))
            .gap(px(4.))
            .rounded(px(5.))
            .flex_shrink_0()
            .items_center()
            .text_size(px(13.))
            .text_color(m.text_muted)
            .cursor_pointer()
            .hover(|d| d.bg(m.hover).text_color(m.text))
            .child(disclosure(
                "playlists-disclosure",
                (!playlists.is_empty()).then_some(open),
                &m,
            ))
            .child(
                Icon::new(IconName::ListMusic)
                    .size(px(16.))
                    .flex_shrink_0()
                    .mr(px(2.))
                    .text_color(m.text_faint),
            )
            .child(div().flex_1().min_w_0().truncate().child("Playlists"))
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
            .on_click(cx.listener(|s, _, _, cx| {
                s.playlists_open = !s.playlists_open;
                cx.emit(SidebarEvent::Changed);
                cx.notify();
            }))
            .context_menu(|menu, _, _| {
                menu.item(
                    PopupMenuItem::new("New Playlist")
                        .icon(IconName::Plus)
                        .action(Box::new(crate::workspace::NewPlaylist)),
                )
            })
            .into_any_element();
        let mut rows = vec![header];
        if !open {
            return rows;
        }
        for p in playlists {
            let id = p.id;
            let selected = location == Location::Playlist(id);
            if let Some((_, input)) = self.renaming_playlist.as_ref().filter(|(r, _)| *r == id) {
                // The label gives way to the name field.
                rows.push(
                    h_flex()
                        .h(px(ROW))
                        .mx(px(6.))
                        .pl(px(4. + INDENT))
                        .pr(px(8.))
                        .gap(px(4.))
                        .flex_shrink_0()
                        .items_center()
                        .key_context("LocationBar")
                        .on_action(cx.listener(|s, _: &crate::location::CancelEdit, w, cx| {
                            if s.renaming_playlist.take().is_some() {
                                s.focus_list(w, cx);
                                cx.notify();
                            }
                        }))
                        .child(div().size(px(20.)).flex_shrink_0())
                        .child(
                            Icon::new(IconName::Music2)
                                .size(px(15.))
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
                1,
                Some(Icon::new(IconName::Music2)),
                p.name.clone().into(),
                selected,
                &m,
            );
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
    player: Entity<crate::player::Player>,
    this: WeakEntity<Sidebar>,
) -> PopupMenu {
    let play = move |next: Option<bool>, cx: &mut App| {
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
                    None => queue_targets(&catalog_, &player, targets, |t| Command::Play(t, 0), cx),
                    Some(true) => queue_targets(&catalog_, &player, targets, Command::PlayNext, cx),
                    Some(false) => queue_targets(&catalog_, &player, targets, Command::Enqueue, cx),
                }
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

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let missing = self.places.missing_defaults();
        let (heading, empty) = (cx.weak_entity(), cx.weak_entity());
        let end_drop = cx.listener(|s, d: &DraggedPlace, _, cx| {
            let end = s.places.items.len();
            s.places.move_item(d.ix, end);
            s.changed(cx);
        });
        v_flex()
            .size_full()
            .bg(m.chrome)
            .pt(px(2.))
            // The library keeps at most half the height, so a long list of
            // playlists never buries Locations, and a deep folder tree
            // scrolls on its own below it.
            .child(
                v_flex()
                    .flex_shrink_0()
                    .max_h(relative(0.5))
                    .child(
                        section("library-heading", "Library", cx)
                            .context_menu(|menu, _, _| Self::library_menu(menu)),
                    )
                    .child(
                        v_flex()
                            .id("library")
                            .min_h_0()
                            .pb(px(4.))
                            .gap(px(1.))
                            .overflow_y_scroll()
                            .children(self.render_library(cx)),
                    ),
            )
            .child(divider(cx))
            .child(
                section("locations-heading", "Locations", cx)
                    // Dropping a Place on the heading moves it to the top.
                    .drag_over::<DraggedPlace>(move |d, _, _, _| {
                        d.border_b_2().border_color(m.accent)
                    })
                    .on_drop(cx.listener(|s, d: &DraggedPlace, _, cx| {
                        s.places.move_item(d.ix, 0);
                        s.changed(cx);
                    }))
                    .context_menu(move |menu, _, _| {
                        Self::locations_menu(heading.clone(), menu, missing)
                    }),
            )
            .child(
                div()
                    .id("locations")
                    .flex_1()
                    .min_h_0()
                    .pb(px(6.))
                    // Space below the last row takes a Place to the end.
                    .drag_over::<DraggedPlace>(move |d, _, _, _| {
                        d.border_b_2().border_color(m.accent.opacity(0.6))
                    })
                    .on_drop(end_drop)
                    .context_menu(move |menu, _, _| {
                        Self::locations_menu(empty.clone(), menu, missing)
                    })
                    .child(self.render_locations(cx)),
            )
    }
}
