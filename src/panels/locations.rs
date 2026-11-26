//! Locations: the file system. Every Place is the root of its own folder
//! hierarchy and expands in place, so the folders of Music appear under
//! Music rather than in a second tree.
//!
//! The Places, their availability and the folder tree are shared state in
//! [`Locations`]: closing the panel forgets nothing, and the tree follows
//! the active Music Browser whether or not the panel is shown.
use super::{
    Destination, PaneContent, PanelType, Router, Services,
    nav::{DraggedFolder, INDENT, ROW},
};
use crate::disclosure::{Disclosure, VirtualFocus};
use crate::{
    assets::FileIcon,
    catalog::Catalog,
    folders::{ExpansionChanged, Folders},
    places::{Known, Origin, Places, PlacesState, same_path},
    theme::materials,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, Sizable, WindowExt, h_flex,
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
use std::{path::PathBuf, rc::Rc, sync::Arc};

pub static TYPE: PanelType = PanelType {
    key: "disqro.locations",
    title: "Locations",
    icon: IconName::FolderOpen,
    instances: super::Instances::Single,
    home: super::Home::Leading,
    build: |services, _, window, cx| {
        let panel = cx.new(|cx| LocationsPanel::new(services, window, cx));
        Rc::new(panel)
    },
};

/// The Places changed, or folders were opened or closed: save the workspace.
pub struct LocationsChanged;

/// The Places and the folder tree beneath them.
pub struct Locations {
    pub places: Places,
    available: std::collections::HashMap<PathBuf, bool>,
    availability_task: Option<Task<()>>,
    _expansion: Subscription,
    pub folders: Entity<Folders>,
}
impl EventEmitter<LocationsChanged> for Locations {}

impl Locations {
    pub fn new(saved: Option<PlacesState>, expanded: Vec<String>, cx: &mut Context<Self>) -> Self {
        let folders = cx.new(Folders::new);
        folders.update(cx, |f, _| f.restore_expanded(expanded));
        let expansion = cx.subscribe(&folders, |_, _, _: &ExpansionChanged, cx| {
            cx.emit(LocationsChanged)
        });
        let mut this = Self {
            places: Places::restore(saved),
            available: Default::default(),
            availability_task: None,
            _expansion: expansion,
            folders,
        };
        this.check_availability(cx);
        this.share_places(cx);
        this
    }
    pub fn expanded(&self, cx: &App) -> Vec<String> {
        self.folders.read(cx).expanded_state()
    }
    pub fn add(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.places.add(path) {
            self.changed(cx);
        }
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.check_availability(cx);
        self.share_places(cx);
        cx.emit(LocationsChanged);
        cx.notify();
    }
    /// Probes places off the interface thread; a sleeping network share must
    /// not stall the window.
    fn check_availability(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.places.items.iter().map(|p| p.path.clone()).collect();
        self.availability_task = Some(cx.spawn(async move |this, cx| {
            let available = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .map(|p| {
                            let available = p.is_dir();
                            (p, available)
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |s, cx| {
                s.available = available;
                cx.notify();
            });
        }));
    }
    /// Every Place is a root of the tree; drives yield to the other Places
    /// when choosing where to show a folder.
    fn share_places(&self, cx: &mut App) {
        let places = self
            .places
            .items
            .iter()
            .map(|p| (p.path.clone(), p.origin == Origin::Drive))
            .collect();
        self.folders.update(cx, |f, cx| f.set_places(places, cx));
    }
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

/// What a row needs to know about the Places, captured when the panel renders.
#[derive(Clone)]
struct PlaceRow {
    path: PathBuf,
    name: SharedString,
    origin: Origin,
    available: bool,
}

pub struct LocationsPanel {
    model: Entity<Locations>,
    router: Entity<Router>,
    catalog: Entity<Catalog>,
    renaming: Option<(PathBuf, Entity<InputState>)>,
    row_focus: Rc<VirtualFocus<(SharedString, bool)>>,
    _model_subscription: Subscription,
    focus: FocusHandle,
}

impl LocationsPanel {
    fn new(services: &Services, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let model_subscription = cx.observe(&services.locations, |_, _, cx| cx.notify());
        Self {
            model: services.locations.clone(),
            router: services.router.clone(),
            catalog: services.catalog.clone(),
            renaming: None,
            _model_subscription: model_subscription,
            row_focus: Default::default(),
            focus: cx.focus_handle(),
        }
    }
    fn folders(&self, cx: &App) -> Entity<Folders> {
        self.model.read(cx).folders.clone()
    }
    fn start_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(place) = self
            .model
            .read(cx)
            .places
            .items
            .iter()
            .find(|p| same_path(&p.path, &path))
        else {
            return;
        };
        let name = place.name();
        let editing = path.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        cx.subscribe_in(&input, window, move |s, input, e, window, cx| match e {
            InputEvent::PressEnter { .. } | InputEvent::Blur
                if s.renaming.as_ref().is_some_and(|(p, _)| *p == editing) =>
            {
                let label = input.read(cx).value().to_string();
                s.renaming = None;
                s.model.update(cx, |m, cx| {
                    if let Some(ix) = m.places.position(&editing) {
                        m.places.rename(ix, &label);
                        m.changed(cx);
                    }
                });
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
        self.renaming = Some((path, input));
        cx.notify();
    }
    fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
        }
    }
    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add to Places".into()),
        });
        let model = self.model.clone();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(paths))) = result.await
                && let Some(path) = paths.into_iter().next()
            {
                model.update(cx, |m, cx| m.add(path, cx));
            }
        })
        .detach();
    }
    fn confirm_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title("Reset Places to defaults?")
                .description(
                    "Replace your shortcuts, names and order with this computer’s default Places?",
                )
                .show_cancel(true)
                .ok_text("Reset Places")
                .on_ok(move |_, _, cx| {
                    let _ = this.update(cx, |s, cx| {
                        s.renaming = None;
                        s.model.update(cx, |m, cx| {
                            m.places.reset();
                            m.changed(cx);
                        });
                    });
                    true
                })
        });
    }
    /// Opens the folder an entry shows, unless the active browser shows it.
    fn open(
        router: &Entity<Router>,
        folders: &Entity<Folders>,
        root: PathBuf,
        path: PathBuf,
        new_tab: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let shown = folders
            .read(cx)
            .current()
            .is_some_and(|(r, c)| same_path(r, &root) && same_path(c, &path));
        if !new_tab
            && shown
            && let Some(b) = router.read(cx).active()
            && b.read(cx).location.is_folder()
            && !b.read(cx).is_page()
        {
            router.update(cx, |_, cx| {
                cx.emit(super::RouterEvent::Reveal(b.entity_id()))
            });
        } else {
            let destination = Destination::Folder {
                path,
                root: Some(root),
            };
            Router::open(router, destination, new_tab, window, cx);
        }
    }
    fn panel_menu(this: WeakEntity<Self>, menu: PopupMenu, missing: usize) -> PopupMenu {
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
                    s.model.update(cx, |m, cx| {
                        m.places.restore_missing();
                        m.changed(cx);
                    })
                });
            }),
        )
        .item(
            PopupMenuItem::new("Reset Places to Defaults…").on_click(move |_, w, cx| {
                let _ = c.update(cx, |s, cx| s.confirm_reset(w, cx));
            }),
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn place_menu(
        this: WeakEntity<Self>,
        router: Entity<Router>,
        folders: Entity<Folders>,
        catalog: Entity<Catalog>,
        path: PathBuf,
        menu: PopupMenu,
    ) -> PopupMenu {
        let (b, c) = (this.clone(), this);
        let (open, tab, library, rename) = (path.clone(), path.clone(), path.clone(), path.clone());
        let (r1, f1) = (router.clone(), folders.clone());
        let menu = menu.item(
            PopupMenuItem::new("Open")
                .icon(IconName::FolderOpen)
                .on_click(move |_, w, cx| {
                    Self::open(&r1, &f1, open.clone(), open.clone(), false, w, cx)
                }),
        );
        Destination::Folder {
            path: tab.clone(),
            root: Some(tab),
        }
        .open_items(menu, router)
        .item(
            PopupMenuItem::new("Add to Library")
                .icon(IconName::Library)
                .on_click(move |_, _, cx| {
                    catalog.update(cx, |c, cx| c.add_root(library.clone(), cx))
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new("Rename Shortcut")
                .icon(IconName::Pencil)
                .on_click(move |_, w, cx| {
                    let _ = b.update(cx, |s, cx| s.start_rename(rename.clone(), w, cx));
                }),
        )
        .item(
            PopupMenuItem::new("Remove from Places")
                .icon(IconName::X)
                .on_click(move |_, _, cx| {
                    let _ = c.update(cx, |s, cx| {
                        s.renaming = None;
                        s.model.update(cx, |m, cx| {
                            if let Some(ix) = m.places.position(&path) {
                                m.places.remove(ix);
                                m.changed(cx);
                            }
                        });
                    });
                }),
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn folder_menu(
        model: Entity<Locations>,
        router: Entity<Router>,
        folders: Entity<Folders>,
        catalog: Entity<Catalog>,
        root: PathBuf,
        path: PathBuf,
        pinned: bool,
        menu: PopupMenu,
    ) -> PopupMenu {
        let (open, tab, add, library) = (path.clone(), path.clone(), path.clone(), path);
        let (r1, f1, root1) = (router.clone(), folders.clone(), root.clone());
        let menu = menu.item(
            PopupMenuItem::new("Open")
                .icon(IconName::FolderOpen)
                .on_click(move |_, w, cx| {
                    Self::open(&r1, &f1, root1.clone(), open.clone(), false, w, cx)
                }),
        );
        Destination::Folder {
            path: tab,
            root: Some(root),
        }
        .open_items(menu, router)
        .separator()
        .item(
            PopupMenuItem::new(if pinned {
                "Already in Places"
            } else {
                "Add to Places"
            })
            .icon(IconName::Pin)
            .disabled(pinned)
            .on_click(move |_, _, cx| model.update(cx, |m, cx| m.add(add.clone(), cx))),
        )
        .item(
            PopupMenuItem::new("Add to Library")
                .icon(IconName::Library)
                .on_click(move |_, _, cx| {
                    catalog.update(cx, |c, cx| c.add_root(library.clone(), cx))
                }),
        )
    }

    /// One tree whose roots are the Places. The disclosure only opens or
    /// closes a branch; the name opens the folder. Both resolve their entry
    /// by root and path, so a click that lands while the tree updates still
    /// means the folder it showed.
    fn render_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let model = self.model.read(cx);
        let folders = model.folders.clone();
        let state = folders.read(cx).state.clone();
        let places: Arc<[PlaceRow]> = model
            .places
            .items
            .iter()
            .map(|p| PlaceRow {
                path: p.path.clone(),
                name: p.name().into(),
                origin: p.origin.clone(),
                available: model.available.get(&p.path).copied().unwrap_or(true),
            })
            .collect();
        let renaming = self.renaming.clone();
        let (router, panel) = (self.router.clone(), cx.weak_entity());
        let menu_places = places.clone();
        let (menu_folders, menu_router, menu_panel, menu_model, catalog) = (
            folders.clone(),
            self.router.clone(),
            cx.weak_entity(),
            self.model.clone(),
            self.catalog.clone(),
        );
        let model_handle = self.model.clone();
        let focus = self.row_focus.clone();
        tree(&state, move |_, entry, selected, _, cx| {
            let depth = entry.depth();
            let id = entry.item().id.clone();
            let row_id = ElementId::Name(id.clone());
            let Some((root, path)) = folders.read(cx).entry(&id) else {
                return ListItem::new(row_id).h(px(ROW));
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
            let expander = if entry.is_folder() || !available {
                let toggle = folders.clone();
                let toggle_id = id.clone();
                Disclosure::new("location-disclosure", entry.is_expanded(), label.clone())
                    .track_focus(focus.handle((id.clone(), true), cx))
                    .disabled(!available)
                    .on_toggle(move |_, cx| toggle.update(cx, |f, cx| f.toggle(&toggle_id, cx)))
                    .into_any_element()
            } else {
                Disclosure::slot().into_any_element()
            };
            let content = h_flex()
                .id("location-row")
                .size_full()
                .gap(px(4.))
                .min_w_0()
                .items_center()
                .text_size(px(crate::theme::size::TEXT))
                .text_color(if selected {
                    m.text
                } else if temporary {
                    m.text_faint
                } else {
                    m.text_muted
                })
                .pl(px(10. + depth as f32 * INDENT))
                .pr(px(14.))
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
                .filter(|(p, _)| depth == 0 && same_path(p, &root))
                .map(|(_, input)| input.clone());
            let content = if let Some(input) = editing {
                let s = panel.clone();
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
                let (r, f) = (router.clone(), folders.clone());
                let (open_root, open_path) = (root.clone(), path.clone());
                let row_focus = focus.handle((id.clone(), false), cx);
                let open = move |e: &ClickEvent, window: &mut Window, cx: &mut App| {
                    // The tree's own click handling would select by row
                    // position and toggle expansion. Navigate by path instead.
                    cx.stop_propagation();
                    let new_tab = e.modifiers().secondary(cx);
                    if !new_tab {
                        let state = f.read(cx).state.clone();
                        state.update(cx, |t, cx| {
                            if !matches!(e, ClickEvent::Keyboard(_)) {
                                t.focus(window, cx);
                            }
                            t.set_selected_index(t.index_of(&id), cx);
                        });
                    }
                    Self::open(
                        &r,
                        &f,
                        open_root.clone(),
                        open_path.clone(),
                        new_tab,
                        window,
                        cx,
                    );
                };
                let dragged = DraggedFolder {
                    path: path.clone(),
                    label: label.clone(),
                    place: place.is_some(),
                };
                content
                    .role(Role::Button)
                    .aria_label(format!("Open {label}"))
                    .aria_selected(selected)
                    .key_context("NavigationRow")
                    .track_focus(&row_focus)
                    .tab_stop(true)
                    .focus_visible(|d| d.bg(m.control).text_color(m.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_ellipsis_middle()
                            .child(label.clone()),
                    )
                    .on_click(open)
                    .when(available && !temporary, |d| {
                        let tip = path.display().to_string();
                        d.tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                    })
                    .when(!available, |d| {
                        let tip = format!("{} is not available", path.display());
                        d.tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                    })
                    .when(temporary, |d| {
                        let tip = format!("{} is not in Places", root.display());
                        d.tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                    })
                    .when(available && !temporary, |d| {
                        d.on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                    })
                    .when(place.is_some(), |d| {
                        let destination = path.clone();
                        // Dropping a Place onto another moves it there.
                        let model = model_handle.clone();
                        d.drag_over::<DraggedFolder>(move |d, dragged, _, _| {
                            if dragged.place {
                                d.border_t_2().border_color(m.accent)
                            } else {
                                d
                            }
                        })
                        .on_drop(move |d: &DraggedFolder, _, cx| {
                            if d.place {
                                model.update(cx, |m, cx| {
                                    if let (Some(from), Some(to)) = (
                                        m.places.position(&d.path),
                                        m.places.position(&destination),
                                    ) {
                                        m.places.move_item(from, to);
                                        m.changed(cx);
                                    }
                                });
                            }
                        })
                    })
            };
            ListItem::new(row_id)
                .h(px(ROW))
                .p_0()
                .rounded(px(5.))
                .child(content)
        })
        .context_menu(move |_, entry, menu, _, cx| {
            let Some((root, path)) = menu_folders.read(cx).entry(&entry.item().id) else {
                return menu;
            };
            if entry.depth() == 0 && menu_places.iter().any(|p| same_path(&p.path, &root)) {
                return Self::place_menu(
                    menu_panel.clone(),
                    menu_router.clone(),
                    menu_folders.clone(),
                    catalog.clone(),
                    path,
                    menu,
                );
            }
            let pinned = menu_places.iter().any(|p| same_path(&p.path, &path));
            Self::folder_menu(
                menu_model.clone(),
                menu_router.clone(),
                menu_folders.clone(),
                catalog.clone(),
                root,
                path,
                pinned,
                menu,
            )
        })
    }
}

impl Render for LocationsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let menu_model = self.model.clone();
        let (menu_this, model) = (cx.weak_entity(), self.model.clone());
        let end_drop = move |d: &DraggedFolder, _: &mut Window, cx: &mut App| {
            if d.place {
                model.update(cx, |m, cx| {
                    let end = m.places.items.len();
                    if let Some(from) = m.places.position(&d.path) {
                        m.places.move_item(from, end);
                        m.changed(cx);
                    }
                });
            }
        };
        v_flex()
            .id("locations")
            .key_context("Locations")
            .track_focus(&self.focus)
            .size_full()
            .bg(m.chrome)
            .pt(px(4.))
            .on_action(
                cx.listener(|s, _: &crate::workspace::OpenDirectory, w, cx| {
                    let folders = s.folders(cx);
                    let item = folders.read(cx).state.read(cx).selected_item().cloned();
                    if let Some((root, path)) = item.and_then(|i| folders.read(cx).entry(&i.id)) {
                        Self::open(&s.router, &folders, root, path, false, w, cx);
                    }
                }),
            )
            .child(
                div()
                    .id("locations-tree")
                    .flex_1()
                    .min_h_0()
                    .pb(px(6.))
                    // Space below the last row takes a Place to the end.
                    .drag_over::<DraggedFolder>(move |d, dragged, _, _| {
                        if dragged.place {
                            d.border_b_2().border_color(m.accent.opacity(0.6))
                        } else {
                            d
                        }
                    })
                    .on_drop(end_drop)
                    .context_menu(move |menu, _, cx| {
                        Self::panel_menu(
                            menu_this.clone(),
                            menu,
                            menu_model.read(cx).places.missing_defaults(),
                        )
                    })
                    .child(self.render_tree(cx)),
            )
    }
}

impl PaneContent for Entity<LocationsPanel> {
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
    fn focused(&self, window: &mut Window, cx: &mut App) {
        // Arrow keys move through the folders once the panel has focus.
        let panel = self.read(cx);
        if panel.focus.is_focused(window) {
            let state = panel.folders(cx).read(cx).state.clone();
            state.update(cx, |t, cx| t.focus(window, cx));
        }
    }
}
