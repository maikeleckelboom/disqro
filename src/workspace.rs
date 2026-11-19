use crate::{
    assets::FileIcon,
    audio::Command,
    browser::{Browser, music_home},
    library::folder_name,
    player::Player,
    profile::{self, Profile},
};
use gpui_kit::{
    component::{
        button::*,
        dock::*,
        input::Input,
        list::ListItem,
        menu::{DropdownMenu, PopupMenuItem},
        status_bar::StatusBar,
        table::DataTable,
        tree::tree,
        *,
    },
    prelude::*,
    *,
};
use std::path::PathBuf;

gpui_kit::actions!(
    disqro,
    [
        OpenFolder,
        FindTrack,
        PlayPause,
        PlaySelected,
        OpenDirectory,
        GoBack
    ]
);

#[derive(Copy, Clone, PartialEq, Eq)]
enum Kind {
    Folders,
    Tracks,
    Player,
}
impl Kind {
    const ALL: [Self; 3] = [Self::Folders, Self::Tracks, Self::Player];
    fn name(self) -> &'static str {
        match self {
            Self::Folders => "Folders",
            Self::Tracks => "Recordings",
            Self::Player => "Now playing",
        }
    }
}

struct Pane {
    kind: Kind,
    browser: Entity<Browser>,
    player: Entity<Player>,
    focus: FocusHandle,
}
impl EventEmitter<PanelEvent> for Pane {}
impl Focusable for Pane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl BasePanel for Pane {
    fn panel_name(&self) -> &'static str {
        self.kind.name()
    }
}
impl Panel for Pane {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.kind.name()
    }
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.kind.name().into())
    }
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}
impl Pane {
    fn folders(&self, cx: &App) -> impl IntoElement {
        let b = self.browser.read(cx);
        let browser = self.browser.clone();
        let tree_view = tree(&b.tree, move |ix, entry, _, _, cx| {
            let root = entry.depth() == 0;
            let id = entry.item().id.clone();
            let browser = browser.clone();
            ListItem::new(("folder", ix))
                .px_2()
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .pl(px(entry.depth() as f32 * 12.))
                        .when(root && entry.is_folder(), |row| {
                            row.child(
                                Icon::new(if entry.is_expanded() {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .xsmall(),
                            )
                        })
                        .child(
                            FileIcon::Folder
                                .icon()
                                .small()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(div().truncate().child(entry.item().label.clone())),
                )
                .on_click(move |_, window, cx| {
                    if !root && let Some(path) = browser.read(cx).tree_path(&id) {
                        browser.update(cx, |b, cx| b.open(path, window, cx));
                    }
                })
        });
        let mut places = v_flex().gap_1();
        let home = music_home();
        let root = self
            .browser
            .read(cx)
            .path
            .ancestors()
            .last()
            .map(|p| p.to_owned());
        let mut shortcuts = vec![("Music".to_owned(), home, FileIcon::Music)];
        if let Some(home) = std::env::var_os("USERPROFILE") {
            shortcuts.push((
                "Downloads".to_owned(),
                PathBuf::from(home).join("Downloads"),
                FileIcon::Folder,
            ));
        }
        if let Some(root) = root {
            shortcuts.push((format!("Drive {}", root.display()), root, FileIcon::Drive));
        }
        for (i, (label, path, icon)) in shortcuts.into_iter().enumerate() {
            let browser = self.browser.clone();
            places = places.child(
                ListItem::new(("place", i))
                    .px_2()
                    .child(h_flex().gap_2().child(icon.icon().small()).child(label))
                    .on_click(move |_, window, cx| {
                        browser.update(cx, |b, cx| b.open(path.clone(), window, cx))
                    }),
            );
        }
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                v_flex()
                    .px_2()
                    .py_3()
                    .gap_1()
                    .child(
                        div()
                            .px_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Places"),
                    )
                    .child(places),
            )
            .child(div().border_t_1().border_color(cx.theme().border).mx_3())
            .child(
                div()
                    .px_4()
                    .pt_3()
                    .pb_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Current folder"),
            )
            .child(div().flex_1().min_h_0().px_1().child(tree_view))
    }
    fn recordings(&self, cx: &App) -> impl IntoElement {
        let b = self.browser.read(cx);
        let table = b.table.read(cx).delegate();
        let empty = table.rows.is_empty();
        let mut view = v_flex().size_full().child(
            h_flex()
                .px_4()
                .py_3()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_xl()
                        .child(b.album.clone().unwrap_or_else(|| folder_name(&b.path))),
                )
                .child(
                    div().w(px(185.)).child(
                        Input::new(&b.search)
                            .small()
                            .prefix(Icon::new(IconName::Search).small())
                            .cleanable(true),
                    ),
                ),
        );
        if let Some(notice) = &b.notice {
            view = view.child(
                div()
                    .px_4()
                    .py_1()
                    .text_sm()
                    .text_color(cx.theme().warning)
                    .child(notice.clone()),
            );
        }
        if empty {
            let label = if b.busy {
                "Opening folder…"
            } else if !b.search.read(cx).value().is_empty() {
                "No matching recordings"
            } else if b.folder_count > 0 {
                "Choose a subfolder to browse its recordings"
            } else {
                "No supported recordings in this folder"
            };
            view.child(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .text_color(cx.theme().muted_foreground)
                    .child(Icon::new(IconName::FolderOpen).size(px(30.)))
                    .child(label),
            )
        } else {
            view.child(
                div().flex_1().min_h_0().child(
                    DataTable::new(&b.table)
                        .accessibility_label("Recordings")
                        .bordered(false)
                        .small(),
                ),
            )
        }
    }
}
impl Render for Pane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.kind {
            Kind::Folders => self.folders(cx).into_any_element(),
            Kind::Tracks => self.recordings(cx).into_any_element(),
            Kind::Player => self.player.clone().into_any_element(),
        };
        div()
            .track_focus(&self.focus)
            .size_full()
            .overflow_hidden()
            .child(content)
    }
}

pub struct Workspace {
    browser: Entity<Browser>,
    player: Entity<Player>,
    dock: Entity<DockArea>,
    panels: Vec<Entity<Pane>>,
    profile: Option<PathBuf>,
    profile_notice: Option<String>,
}
impl Workspace {
    pub fn new(arg: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut location = profile::location();
        let mut profile_notice = None;
        let mut saved = location
            .as_deref()
            .and_then(|p| match profile::read(p) {
                Ok(s) => Some(s),
                Err(e) => {
                    profile_notice = Some(format!("Could not restore workspace · {e}"));
                    None
                }
            })
            .unwrap_or_default();
        if profile_notice.is_some() {
            location = None;
        }
        let volume = saved.volume.clamp(0., 1.);
        let player = cx.new(|cx| Player::new(volume, window, cx));
        let browser = cx.new(|cx| Browser::new(player.clone(), saved.columns, window, cx));
        let panels = Kind::ALL
            .into_iter()
            .map(|kind| {
                let pane = cx.new(|cx| {
                    if kind != Kind::Player {
                        cx.observe(&browser, |_, _, cx| cx.notify()).detach();
                    }
                    Pane {
                        kind,
                        browser: browser.clone(),
                        player: player.clone(),
                        focus: cx.focus_handle(),
                    }
                });
                let factory = pane.clone();
                register_panel(cx, kind.name(), move |_, _, _| {
                    panel_handle(factory.clone())
                });
                pane
            })
            .collect::<Vec<_>>();
        let (dock, _) = DockSkin::dock_area("listening", Some(1), window, cx);
        let mut app = Self {
            browser,
            player,
            dock,
            panels,
            profile: location,
            profile_notice,
        };
        if !saved.layout.take().is_some_and(|layout| {
            layout.version == Some(1)
                && app
                    .dock
                    .update(cx, |d, cx| d.load(layout, window, cx))
                    .is_ok()
        }) {
            app.reset(window, cx);
        }
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
        app.browser.update(cx, |b, cx| {
            b.load(folder, is_file.then_some(target), window, cx)
        });
        cx.observe(&app.browser, |_, _, cx| cx.notify()).detach();
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = this.update(cx, |s, cx| s.save(cx));
            true
        });
        app
    }
    fn save(&self, cx: &App) {
        let Some(path) = &self.profile else { return };
        let b = self.browser.read(cx);
        let s = Profile {
            folder: Some(b.path.clone()),
            volume: self.player.read(cx).volume,
            columns: b.table.read(cx).delegate().columns.clone(),
            layout: Some(self.dock.read(cx).dump(cx)),
        };
        if let Err(e) = profile::save(path, &s) {
            eprintln!("Workspace save failed: {e}");
        }
    }
    fn reset(&mut self, w: &mut Window, cx: &mut App) {
        let tab =
            |i: usize| DockLayout::tabs().panel_view(panel_handle(self.panels[i].clone()), cx);
        let right = DockLayout::v_split()
            .child(tab(1), None)
            .child(tab(2), Some(px(158.)));
        let layout = DockLayout::h_split()
            .child(tab(0), Some(px(215.)))
            .child(right, None);
        self.dock.update(cx, |d, cx| d.set_center(layout, w, cx));
    }
    fn show(&self, index: usize, w: &mut Window, cx: &mut App) {
        let panel = panel_handle(self.panels[index].clone());
        self.dock.update(cx, |d, cx| {
            let id = panel.panel_id(cx);
            if d.panel(id).is_some() {
                d.select_panel(id, w, cx);
            } else {
                d.add_panel_view(panel, DockPlacement::Center, None, w, cx);
            }
        });
    }
    fn play_selected(&self, cx: &mut App) {
        let t = self.browser.read(cx).table.read(cx);
        if let Some(ix) = t
            .selected_row()
            .or_else(|| (!t.delegate().rows.is_empty()).then_some(0))
        {
            self.browser
                .read(cx)
                .table
                .clone()
                .update(cx, |t, cx| t.delegate().play(ix, cx));
        }
    }
}
impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let b = self.browser.read(cx);
        let mut status = format!("{} recordings", b.table.read(cx).delegate().rows.len());
        if b.folder_count > 0 {
            status = format!("{status}  ·  {} folders", b.folder_count);
        }
        let has_history = !b.history.is_empty();
        let path = b.path.display().to_string();
        let parent = b.path.parent().map(|p| p.to_owned());
        let shell = cx.weak_entity();
        let menu = Button::new("workspace")
            .ghost()
            .small()
            .icon(Icon::new(gpui_kit::assets::IconName::PanelsTopLeft))
            .accessibility_label("Workspace")
            .tooltip("Workspace")
            .dropdown_menu(move |mut menu, _, _| {
                for (i, kind) in Kind::ALL.iter().enumerate() {
                    let s = shell.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("Show {}", kind.name())).on_click(
                            move |_, w, cx| {
                                let _ = s.update(cx, |s, cx| s.show(i, w, cx));
                            },
                        ),
                    );
                }
                let s = shell.clone();
                menu.separator()
                    .item(
                        PopupMenuItem::new("Restore default layout").on_click(move |_, w, cx| {
                            let _ = s.update(cx, |s, cx| s.reset(w, cx));
                        }),
                    )
            });
        v_flex()
            .key_context("Disqro")
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(
                cx.listener(|s, _: &OpenFolder, w, cx| {
                    s.browser.update(cx, |b, cx| b.choose(w, cx))
                }),
            )
            .on_action(
                cx.listener(|s, _: &GoBack, w, cx| s.browser.update(cx, |b, cx| b.back(w, cx))),
            )
            .on_action(cx.listener(|s, _: &FindTrack, w, cx| {
                s.show(1, w, cx);
                s.browser
                    .read(cx)
                    .search
                    .read(cx)
                    .focus_handle(cx)
                    .focus(w, cx);
            }))
            .on_action(cx.listener(|s, _: &PlayPause, _, cx| {
                if s.player.read(cx).status.track.is_none() {
                    s.play_selected(cx);
                } else {
                    s.player.update(cx, |p, cx| p.command(Command::Toggle, cx));
                }
            }))
            .on_action(cx.listener(|s, _: &PlaySelected, _, cx| s.play_selected(cx)))
            .on_action(cx.listener(|s, _: &OpenDirectory, w, cx| {
                let b = s.browser.read(cx);
                if let Some(path) = b
                    .tree
                    .read(cx)
                    .selected_item()
                    .and_then(|i| b.tree_path(&i.id))
                    && path != b.path
                {
                    s.browser.update(cx, |b, cx| b.open(path, w, cx));
                }
            }))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .gap_2()
                        .px_2()
                        .child(img("brand/compact.png").size(px(22.)))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_sm()
                                .child("Disqro"),
                        ),
                ),
            )
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("back")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowLeft)
                            .accessibility_label("Back")
                            .tooltip("Back · Alt+Left")
                            .disabled(!has_history)
                            .on_click(cx.listener(|s, _, w, cx| {
                                s.browser.update(cx, |b, cx| b.back(w, cx))
                            })),
                    )
                    .child(
                        Button::new("up")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowUp)
                            .accessibility_label("Parent folder")
                            .tooltip("Parent folder")
                            .disabled(parent.is_none())
                            .on_click(cx.listener(move |s, _, w, cx| {
                                if let Some(p) = &parent {
                                    s.browser.update(cx, |b, cx| b.open(p.clone(), w, cx));
                                }
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .px_2()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(path),
                    )
                    .child(
                        Button::new("open")
                            .small()
                            .icon(IconName::FolderOpen)
                            .label("Open folder")
                            .tooltip("Open folder · Ctrl+O")
                            .on_click(cx.listener(|s, _, w, cx| {
                                s.browser.update(cx, |b, cx| b.choose(w, cx))
                            })),
                    )
                    .child(menu),
            )
            .child(div().flex_1().min_h_0().child(self.dock.clone()))
            .child(StatusBar::new().left(self.profile_notice.clone().unwrap_or(status)))
    }
}
