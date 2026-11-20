//! The navigation sidebar: the user's Places above the folder tree.
use crate::{
    assets::FileIcon,
    browser::Browser,
    places::{Known, Origin, Place, Places, PlacesState, same_path},
    theme::materials,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        Icon, Sizable, WindowExt, h_flex,
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
    renaming: Option<(usize, Entity<InputState>)>,
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

impl Sidebar {
    pub fn new(
        browser: Entity<Browser>,
        saved: Option<PlacesState>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&browser, |_, _, cx| cx.notify()).detach();
        let mut this = Self {
            places: Places::restore(saved),
            browser,
            renaming: None,
            available: Arc::from([]),
        };
        this.check_availability(cx);
        this
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.check_availability(cx);
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
            self.browser.update(cx, |b, cx| b.open(path, window, cx));
        }
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
        current: &std::path::Path,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let m = *materials(cx);
        let place = &self.places.items[ix];
        let label: SharedString = place.name().into();
        let selected = same_path(&place.path, current);
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
                menu.item(
                    PopupMenuItem::new("Open")
                        .icon(IconName::FolderOpen)
                        .on_click(move |_, w, cx| {
                            let _ = a.update(cx, |s, cx| s.open(ix, w, cx));
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
    fn render_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let b = self.browser.read(cx);
        let browser = self.browser.clone();
        let menu_browser = self.browser.clone();
        let sidebar = cx.weak_entity();
        tree(&b.tree, move |ix, entry, selected, _, _| {
            let depth = entry.depth();
            let id = entry.item().id.clone();
            let browser = browser.clone();
            let expandable = entry.is_folder();
            ListItem::new(("folder", ix))
                .h(px(26.))
                .px(px(10.))
                .py_0()
                .mx(px(6.))
                .rounded(px(5.))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .min_w_0()
                        .items_center()
                        .text_size(px(13.))
                        .text_color(if selected { m.text } else { m.text_muted })
                        .pl(px(depth as f32 * 14.))
                        .child(div().w(px(12.)).flex_shrink_0().when(expandable, |d| {
                            d.child(
                                Icon::new(if entry.is_expanded() {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(12.))
                                .text_color(m.text_faint),
                            )
                        }))
                        .child(
                            FileIcon::Folder
                                .icon()
                                .size(px(15.))
                                .flex_shrink_0()
                                .text_color(if selected { m.accent } else { m.text_faint }),
                        )
                        .child(div().truncate().child(entry.item().label.clone())),
                )
                .on_click(move |_, window, cx| {
                    if let Some(path) = browser.read(cx).tree_path(&id)
                        && !same_path(&path, &browser.read(cx).path)
                    {
                        browser.update(cx, |b, cx| b.open(path, window, cx));
                    }
                })
        })
        .context_menu(move |_, entry, menu, _, cx| {
            let Some(path) = menu_browser.read(cx).tree_path(&entry.item().id) else {
                return menu;
            };
            let pinned = sidebar
                .upgrade()
                .is_some_and(|s| s.read(cx).contains(&path));
            let (b, p, s) = (menu_browser.clone(), path.clone(), sidebar.clone());
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
        })
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let current = self.browser.read(cx).path.clone();
        let missing = self.places.missing_defaults();
        let this = cx.weak_entity();
        let rows = (0..self.places.items.len())
            .map(|ix| self.render_place(ix, &current, cx))
            .collect::<Vec<_>>();
        let end_drop = cx.listener(|s, d: &DraggedPlace, _, cx| {
            let end = s.places.items.len();
            s.places.move_item(d.ix, end);
            s.changed(cx);
        });
        v_flex()
            .size_full()
            .bg(m.chrome)
            .pt(px(2.))
            .child(
                v_flex()
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
                    .context_menu(move |menu, _, _| {
                        Self::section_menu(this.clone(), menu, missing)
                    }),
            )
            .child(div().mx(px(16.)).h(px(1.)).bg(m.line))
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
