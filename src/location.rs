//! Per-browser breadcrumbs and the explicit Go to Folder or File editor.
use crate::{
    browser::{Browser, Location, Reveal, Route},
    theme::materials,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        Icon, Sizable, Size, StyleSized,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputEvent, InputState},
        menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
    },
    prelude::*,
    *,
};
use std::{
    cell::Cell,
    path::{Component, Path, PathBuf, Prefix},
    rc::Rc,
};

gpui_kit::actions!(location, [CancelEdit]);

/// Both fields in a Music Browser's navigation row use its original address height.
pub const NAV_FIELD_HEIGHT: f32 = 28.;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", CancelEdit, Some("LocationBar"))]);
}

pub struct LocationBar {
    browser: Entity<Browser>,
    input: Entity<InputState>,
    editing: Option<(Location, Option<crate::collection::Page>)>,
    pending: bool,
    request: u64,
    error: Option<SharedString>,
    width: Rc<Cell<Pixels>>,
    _view: Subscription,
}

/// Only a physical folder view has a filesystem address. A CD page can be
/// projected from a folder while its navigation identity is the CD itself.
fn physical_address<'a>(
    location: &'a Location,
    page: Option<&crate::collection::Page>,
) -> Option<&'a Path> {
    match (location, page) {
        (Location::Folder(path), None) => Some(path),
        _ => None,
    }
}

#[derive(Clone)]
struct Crumb {
    label: SharedString,
    step: Step,
}

#[derive(Clone)]
enum Step {
    Route(Route),
    /// The current page or a section with no independent destination.
    Label,
}

impl Crumb {
    fn new(label: impl Into<SharedString>, step: Step) -> Self {
        Self {
            label: label.into(),
            step,
        }
    }
}

impl Step {
    fn open(&self, browser: &mut Browser, window: &mut Window, cx: &mut Context<Browser>) {
        match self {
            Self::Route(route) => browser.open_ancestor(route.clone(), window, cx),
            Self::Label => {}
        }
    }
}

fn folder_label(folder: &Path) -> String {
    match folder.components().next_back() {
        Some(Component::Normal(name)) => name.to_string_lossy().into_owned(),
        _ => match folder.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
                    "\\\\{}\\{}",
                    server.to_string_lossy(),
                    share.to_string_lossy()
                ),
                _ => prefix.as_os_str().to_string_lossy().into_owned(),
            },
            _ => folder.display().to_string(),
        },
    }
}

fn view_crumbs(browser: &Browser, cx: &App) -> Vec<Crumb> {
    use crate::collection::Page;
    let routes = browser.ancestors();
    let last = routes.len().saturating_sub(1);
    routes
        .into_iter()
        .enumerate()
        .map(|(index, route)| {
            let label = match (&route.location, &route.page) {
                (_, Some(Page::Disc(_))) | (Location::Album { .. }, _) => browser.crumb_title(cx),
                (_, Some(Page::Release(_))) => browser
                    .release()
                    .map_or_else(|| "Release".into(), |(_, title)| title),
                (Location::Folder(path), _) => folder_label(path),
                (Location::Library(None), _) if index != last => "Library".into(),
                (Location::Collections, _) => "Collections".into(),
                (Location::Playlists, _) => "Playlists".into(),
                (Location::Library(Some(id)), _) => browser
                    .catalog
                    .read(cx)
                    .collection(*id)
                    .map_or_else(|| "Collection".into(), |c| c.root.name.clone()),
                (Location::Release { title, .. }, _) => title.clone(),
                _ => browser.title(cx),
            };
            let step = if index == last {
                Step::Label
            } else {
                Step::Route(route)
            };
            Crumb::new(label, step)
        })
        .collect()
}

/// Turns what the user typed or pasted into an absolute path, or explains
/// why it is not one.
pub fn parse(raw: &str) -> Result<PathBuf, &'static str> {
    let mut s = raw.trim().trim_matches('"').trim().to_owned();
    if s.is_empty() {
        return Err("Enter a folder path");
    }
    // Expand %VARIABLES% the way Explorer does.
    let mut offset = 0;
    while let Some(start) = s[offset..].find('%').map(|i| i + offset) {
        let Some(len) = s[start + 1..].find('%') else {
            break;
        };
        let end = start + len + 2;
        match std::env::var(&s[start + 1..end - 1]) {
            Ok(value) if len > 0 => {
                s.replace_range(start..end, &value);
                offset = start + value.len();
            }
            _ => offset = end,
        }
    }
    let s = s.replace('/', "\\");
    let s = if s.len() == 2 && s.ends_with(':') {
        format!("{s}\\")
    } else {
        s
    };
    let path = PathBuf::from(&s);
    if !path.is_absolute() {
        return Err("Enter a full path, such as C:\\Music or \\\\server\\share");
    }
    Ok(path)
}

enum Target {
    Folder,
    Playable,
    Missing,
    Unavailable(String),
}
fn classify(path: &Path) -> Target {
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => Target::Folder,
        Ok(_)
            if crate::library::is_audio(path)
                || path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("cue")) =>
        {
            Target::Playable
        }
        Ok(_) => Target::Unavailable("This is a file, not a folder".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Target::Missing,
        Err(e) => Target::Unavailable(e.to_string()),
    }
}

impl LocationBar {
    pub fn new(browser: Entity<Browser>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Go to folder or file…"));
        cx.subscribe_in(&input, window, |bar, _, e, window, cx| match e {
            InputEvent::PressEnter { .. } => bar.submit(window, cx),
            InputEvent::Change => {
                bar.request += 1;
                bar.pending = false;
                bar.error = None;
                cx.notify();
            }
            InputEvent::Blur if bar.editing.is_some() => bar.cancel(window, cx),
            _ => {}
        })
        .detach();
        let view = Self::follow(&browser, window, cx);
        Self {
            browser,
            input,
            editing: None,
            pending: false,
            request: 0,
            error: None,
            width: Rc::new(Cell::new(px(0.))),
            _view: view,
        }
    }
    fn follow(
        browser: &Entity<Browser>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(
            browser,
            window,
            |bar, browser, _: &crate::browser::ViewChanged, _, cx| {
                let browser = browser.read(cx);
                // Metadata and table updates also emit ViewChanged. Keep a draft
                // while its browser still shows the destination it was opened from.
                if bar.editing.as_ref().is_some_and(|(location, page)| {
                    *location != browser.location || *page != browser.page
                }) {
                    bar.reset(cx);
                } else {
                    cx.notify();
                }
            },
        )
    }
    fn reset(&mut self, cx: &mut Context<Self>) {
        self.request += 1;
        self.pending = false;
        self.editing = None;
        self.error = None;
        cx.notify();
    }
    /// Shows the path as text with everything selected, ready to replace.
    pub fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let browser = self.browser.read(cx);
        let path = physical_address(&browser.location, browser.page.as_ref())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.pending = false;
        self.request += 1;
        self.editing = Some((browser.location.clone(), browser.page.clone()));
        self.error = None;
        self.input.update(cx, |s, cx| {
            s.set_value(path, window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request += 1;
        let was_focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        self.editing = None;
        self.pending = false;
        self.error = None;
        if was_focused {
            self.focus_list(window, cx);
        }
        cx.notify();
    }
    /// Like Explorer, leaving the address bar returns to the file list, so
    /// the keyboard keeps working.
    fn focus_list(&self, window: &mut Window, cx: &mut App) {
        self.browser.read(cx).list_focus(cx).focus(window, cx);
    }
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request += 1;
        let request = self.request;
        let navigation = self.browser.read(cx).revision();
        let raw = self.input.read(cx).value().to_string();
        let path = match parse(&raw) {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e.into());
                cx.notify();
                return;
            }
        };
        self.pending = true;
        cx.notify();
        // A network path can take seconds to answer; keep the window responsive.
        cx.spawn_in(window, async move |this, cx| {
            let probe = path.clone();
            let target = cx.background_spawn(async move { classify(&probe) }).await;
            let _ = this.update_in(cx, |bar, window, cx| {
                if bar.request != request {
                    return;
                }
                if bar.browser.read(cx).revision() != navigation {
                    bar.cancel(window, cx);
                    return;
                }
                bar.pending = false;
                match target {
                    Target::Folder => {
                        bar.editing = None;
                        bar.browser.update(cx, |b, cx| b.open(path, window, cx));
                        bar.focus_list(window, cx);
                    }
                    Target::Playable => {
                        bar.editing = None;
                        bar.focus_list(window, cx);
                        if let Some(parent) = path.parent().map(Path::to_owned) {
                            bar.browser.update(cx, |b, cx| {
                                b.open_revealing(parent, Some(Reveal::Select(path)), window, cx)
                            });
                        }
                    }
                    Target::Missing => {
                        bar.error =
                            Some(format!("Can't find \u{201c}{}\u{201d}", path.display()).into())
                    }
                    Target::Unavailable(e) => bar.error = Some(e.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn render_crumbs(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let current = self.browser.read(cx);
        let icon = match current.location {
            Location::Playlist(_) | Location::Playlists => Some(IconName::ListMusic),
            Location::Folder(_) => None,
            _ => Some(IconName::Library),
        }
        .filter(|_| self.width.get() >= px(160.));
        let all = view_crumbs(current, cx);
        // Collapse leading folders into a menu when the path does not fit.
        let font_size = px(13.);
        let font = window.text_style().font();
        let measure = |s: &SharedString| {
            window
                .text_system()
                .shape_line(
                    s.clone(),
                    font_size,
                    &[TextRun {
                        len: s.len(),
                        font: font.clone(),
                        color: m.text,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                )
                .width
        };
        let chevron = px(16.);
        let pad = px(14.);
        let available = self.width.get() - px(20.) - if icon.is_some() { px(18.) } else { px(0.) };
        let overflow = px(40.);
        let widths: Vec<Pixels> = all.iter().map(|c| measure(&c.label) + pad).collect();
        let mut hidden = 0;
        if available > px(0.) {
            let first_to_keep = if available >= px(112.) { 2 } else { 1 };
            let mut total: Pixels = widths.iter().copied().sum::<Pixels>()
                + chevron * all.len().saturating_sub(1) as f32;
            while hidden + first_to_keep < all.len() && total > available {
                total -= widths[hidden] + chevron;
                hidden += 1;
            }
            if hidden > 0 {
                total += overflow;
                while hidden + first_to_keep < all.len() && total > available {
                    total -= widths[hidden] + chevron;
                    hidden += 1;
                }
            }
        }
        let browser = self.browser.clone();
        let last = all.len().saturating_sub(1);
        let mut row = h_flex()
            .h_full()
            .min_w_0()
            .items_center()
            .overflow_hidden()
            .when_some(icon, |row, icon| {
                row.child(
                    Icon::new(icon)
                        .size(px(14.))
                        .mr(px(4.))
                        .flex_shrink_0()
                        .text_color(m.text_faint),
                )
            });
        if hidden > 0 {
            let collapsed: Vec<_> = all[..hidden].iter().rev().cloned().collect();
            let browser = browser.clone();
            row = row.child(
                div().occlude().child(
                    Button::new("crumbs-overflow")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Ellipsis).text_color(m.text_muted))
                        .accessibility_label("Earlier locations")
                        .dropdown_menu(move |mut menu, _, _| {
                            for crumb in &collapsed {
                                let step = crumb.step.clone();
                                let browser = browser.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(crumb.label.clone())
                                        .disabled(matches!(step, Step::Label))
                                        .on_click(move |_, w, cx| {
                                            browser.update(cx, |b, cx| step.open(b, w, cx))
                                        }),
                                );
                            }
                            menu
                        }),
                ),
            );
            row = row.child(
                Icon::new(IconName::ChevronRight)
                    .size(px(12.))
                    .text_color(m.text_faint)
                    .mx(px(2.)),
            );
        }
        for (i, c) in all.into_iter().enumerate().skip(hidden) {
            if i > hidden {
                row = row.child(
                    Icon::new(IconName::ChevronRight)
                        .size(px(12.))
                        .text_color(m.text_faint)
                        .flex_shrink_0()
                        .mx(px(2.)),
                );
            }
            let browser = browser.clone();
            let navigates = !matches!(c.step, Step::Label);
            let step = c.step.clone();
            let keyboard = (browser.clone(), step.clone());
            row = row.child(
                div()
                    .id(("crumb", i))
                    .map(|d| {
                        if navigates {
                            d.role(Role::Button).aria_label(c.label.clone())
                        } else {
                            d.role(Role::Label).aria_value(c.label.clone())
                        }
                    })
                    .min_w_0()
                    .when(i == last && available > px(0.), |d| {
                        let budget = available - if hidden > 0 { overflow } else { px(0.) };
                        let cap = if i > hidden { budget * 0.57 } else { budget };
                        d.flex_shrink_0().max_w(cap.max(px(32.)))
                    })
                    .px(px(7.))
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .text_size(font_size)
                    .text_color(if i == last { m.text } else { m.text_muted })
                    .when(navigates, |d| {
                        d.tab_stop(true)
                            .on_key_down(move |e, w, cx| {
                                if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                                    cx.stop_propagation();
                                    keyboard.0.update(cx, |b, cx| keyboard.1.open(b, w, cx));
                                }
                            })
                            .focus_visible(|d| d.bg(m.hover))
                            .hover(|d| d.bg(m.hover).text_color(m.text))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, w, cx| {
                                cx.stop_propagation();
                                browser.update(cx, |b, cx| step.open(b, w, cx));
                            })
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .when(i != last, |d| d.text_ellipsis_middle())
                            .child(c.label.clone()),
                    )
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(c.label.clone()).build(w, cx)
                    }),
            );
        }
        row.into_any_element()
    }
}

impl Render for LocationBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        let width = self.width.clone();
        let measure = canvas(
            move |b, window, _| {
                if (width.get() - b.size.width).abs() > px(0.5) {
                    width.set(b.size.width);
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let base = h_flex()
            .id("location")
            .key_context("LocationBar")
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(NAV_FIELD_HEIGHT))
            .items_center()
            .gap(px(4.))
            .rounded(px(6.))
            .border_1()
            .child(measure)
            .on_action(cx.listener(|bar, _: &CancelEdit, w, cx| bar.cancel(w, cx)));
        if self.editing.is_some() {
            return base
                .bg(m.canvas)
                .border_color(if self.error.is_some() {
                    m.danger.opacity(0.7)
                } else {
                    m.accent.opacity(0.6)
                })
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&self.input)
                            .appearance(false)
                            .small()
                            .prefix(
                                Icon::new(IconName::FolderOpen)
                                    .size(px(14.))
                                    .text_color(m.text_faint),
                            )
                            .aria_label("Folder path"),
                    ),
                )
                .when(self.pending, |d| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(m.text_faint)
                            .pr(px(6.))
                            .child("Opening…"),
                    )
                })
                .when_some(self.error.clone(), |d, e| {
                    d.child(
                        h_flex()
                            .flex_shrink_0()
                            .max_w(relative(0.5))
                            .gap_1()
                            .pr(px(6.))
                            .text_size(px(12.))
                            .text_color(m.danger)
                            .child(Icon::new(IconName::CircleAlert).size(px(13.)))
                            .child(div().truncate().text_ellipsis_middle().child(e)),
                    )
                })
                .into_any_element();
        }
        let browser = self.browser.read(cx);
        let address = physical_address(&browser.location, browser.page.as_ref())
            .map(|path| path.display().to_string());
        base.input_px(Size::Small)
            .bg(m.hover)
            .border_color(gpui::transparent_black())
            .hover(|d| d.border_color(m.control_line))
            .when(address.is_some(), |d| {
                d.cursor_text()
                    .on_click(cx.listener(|bar, _, w, cx| bar.edit(w, cx)))
            })
            .child(self.render_crumbs(window, cx))
            .child(div().flex_1().h_full())
            .context_menu(move |menu, _, _| {
                let physical = address.is_some();
                menu.when_some(address.clone(), |menu, copy| {
                    menu.item(
                        PopupMenuItem::new("Copy Address")
                            .icon(IconName::Copy)
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                            }),
                    )
                })
                .item(
                    PopupMenuItem::new(if physical {
                        "Edit Address"
                    } else {
                        "Go to Folder or File…"
                    })
                    .icon(IconName::Pencil)
                    .action(Box::new(crate::workspace::FocusLocation)),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{folder_label, parse};
    use std::path::{Path, PathBuf};
    #[test]
    fn only_physical_folder_views_offer_an_address() {
        use crate::{
            browser::Location,
            collection::{DiscId, Page},
        };
        let folder = Location::Folder(PathBuf::from(r"C:\Albums"));
        let page = Page::Disc(DiscId::Cue(PathBuf::from(r"C:\Albums\Disc.cue")));
        assert_eq!(
            super::physical_address(&folder, None),
            Some(Path::new(r"C:\Albums"))
        );
        assert_eq!(super::physical_address(&folder, Some(&page)), None);
        for view in [
            Location::Liked,
            Location::Playlist(1),
            Location::Library(None),
            Location::Library(Some(2)),
            Location::Album {
                root: Some(2),
                id: 3,
                title: "Disc".into(),
            },
            Location::Release {
                root: None,
                id: 4,
                title: "Release".into(),
            },
        ] {
            assert_eq!(super::physical_address(&view, None), None);
        }
    }
    #[test]
    fn parses_drive_unc_and_quoted_paths() {
        assert_eq!(
            parse("  \"C:\\Music\\Albums\" ").unwrap(),
            PathBuf::from("C:\\Music\\Albums")
        );
        assert_eq!(parse("D:").unwrap(), PathBuf::from("D:\\"));
        assert_eq!(
            parse("C:/Music/Live").unwrap(),
            PathBuf::from("C:\\Music\\Live")
        );
        assert_eq!(
            parse("\\\\server\\share\\Music").unwrap(),
            PathBuf::from("\\\\server\\share\\Music")
        );
        assert!(parse("Music\\Albums").is_err());
        assert!(parse("").is_err());
        unsafe { std::env::set_var("DISQRO_TEST_ROOT", "E:\\Rips") };
        assert_eq!(
            parse("%DISQRO_TEST_ROOT%\\CD1").unwrap(),
            PathBuf::from("E:\\Rips\\CD1")
        );
    }
    #[test]
    fn breadcrumbs_for_drive_and_share() {
        assert_eq!(folder_label(Path::new(r"C:\")), "C:");
        assert_eq!(folder_label(Path::new(r"C:\Users\Music")), "Music");
        assert_eq!(folder_label(Path::new(r"\\nas\media")), r"\\nas\media");
        assert_eq!(folder_label(Path::new(r"\\nas\media\Rips")), "Rips");
    }
}
