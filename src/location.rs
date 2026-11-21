//! The address bar: breadcrumbs for the current folder that turn into an
//! editable path on click, Ctrl+L or Alt+D.
use crate::{
    browser::{Browser, Location, Reveal},
    theme::materials,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        Icon, Sizable,
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

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", CancelEdit, Some("LocationBar"))]);
}

pub struct LocationBar {
    browser: Entity<Browser>,
    input: Entity<InputState>,
    editing: bool,
    pending: bool,
    request: u64,
    error: Option<SharedString>,
    width: Rc<Cell<Pixels>>,
}

/// One clickable step of the current path.
struct Crumb {
    label: SharedString,
    path: PathBuf,
    step: Step,
}
/// Where a crumb leads.
#[derive(Clone, PartialEq)]
enum Step {
    Folder,
    /// The collection a CD or release page belongs to.
    Collection,
    Release(crate::collection::DiscId, String),
    Current,
}

impl Step {
    fn open(
        &self,
        browser: &mut Browser,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Browser>,
    ) {
        match self {
            Self::Folder => browser.open(path, window, cx),
            Self::Collection => browser.show_collection(window, cx),
            Self::Release(key, title) => browser.open_release(key, title.clone(), window, cx),
            Self::Current => {}
        }
    }
}
fn crumbs(path: &Path) -> Vec<Crumb> {
    let mut out = vec![];
    let mut acc = PathBuf::new();
    for c in path.components() {
        acc.push(c.as_os_str());
        match c {
            Component::Prefix(p) => {
                let label = match p.kind() {
                    Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
                        "\\\\{}\\{}",
                        server.to_string_lossy(),
                        share.to_string_lossy()
                    ),
                    _ => p.as_os_str().to_string_lossy().into_owned(),
                };
                out.push(Crumb {
                    label: label.into(),
                    path: PathBuf::new(),
                    step: Step::Folder,
                });
            }
            Component::RootDir => match out.last_mut() {
                Some(prefix) => prefix.path = acc.clone(),
                None => out.push(Crumb {
                    label: "\\".into(),
                    path: acc.clone(),
                    step: Step::Folder,
                }),
            },
            Component::Normal(n) => out.push(Crumb {
                label: n.to_string_lossy().into_owned().into(),
                path: acc.clone(),
                step: Step::Folder,
            }),
            _ => {}
        }
    }
    out.retain(|c| !c.path.as_os_str().is_empty());
    out
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
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a folder path"));
        cx.subscribe_in(&input, window, |bar, _, e, window, cx| match e {
            InputEvent::PressEnter { .. } => bar.submit(window, cx),
            InputEvent::Change => {
                bar.request += 1;
                bar.pending = false;
                bar.error = None;
                cx.notify();
            }
            InputEvent::Blur if bar.editing && !bar.pending => bar.cancel(window, cx),
            _ => {}
        })
        .detach();
        cx.subscribe_in(
            &browser,
            window,
            |bar, _, _: &crate::browser::ViewChanged, _, cx| {
                bar.request += 1;
                bar.pending = false;
                bar.editing = false;
                bar.error = None;
                cx.notify();
            },
        )
        .detach();
        Self {
            browser,
            input,
            editing: false,
            pending: false,
            request: 0,
            error: None,
            width: Rc::new(Cell::new(px(0.))),
        }
    }
    /// Shows the path as text with everything selected, ready to replace.
    pub fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.browser.read(cx).path.display().to_string();
        self.request += 1;
        self.editing = true;
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
        self.editing = false;
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
                        bar.editing = false;
                        bar.browser.update(cx, |b, cx| b.open(path, window, cx));
                        bar.focus_list(window, cx);
                    }
                    Target::Playable => {
                        bar.editing = false;
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
    /// A library view has no folder path; its crumbs name the view.
    fn render_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        let b = self.browser.read(cx);
        let (icon, section) = match b.location {
            Location::Playlist(_) => (IconName::ListMusic, "Playlists"),
            _ => (IconName::Library, "Library"),
        };
        let parent =
            if let Location::Album { root, .. } | Location::Release { root, .. } = b.location {
                Some((
                    root,
                    root.and_then(|r| b.catalog.read(cx).collection(r))
                        .map_or_else(|| "All Music".to_owned(), |r| r.root.name.clone()),
                ))
            } else {
                None
            };
        // A CD of a release names the release between its collection and itself.
        let release = matches!(b.location, Location::Album { .. })
            .then(|| b.release())
            .flatten();
        let library = self.browser.clone();
        let album_browser = self.browser.clone();
        let release_browser = self.browser.clone();
        let name = match b.location {
            Location::Library(None) => None,
            _ => Some(b.crumb_title(cx)),
        };
        h_flex()
            .h_full()
            .min_w_0()
            .items_center()
            .gap(px(4.))
            .px(px(7.))
            .text_size(px(13.))
            .child(Icon::new(icon).size(px(14.)).text_color(m.text_faint))
            .child(
                div()
                    .id("library-crumb")
                    .when(section == "Library" && name.is_some(), |d| {
                        d.cursor_pointer().on_click(move |_, w, cx| {
                            cx.stop_propagation();
                            library.update(cx, |b, cx| b.show(Location::Library(None), w, cx));
                        })
                    })
                    .text_color(if name.is_some() { m.text_muted } else { m.text })
                    .child(section),
            )
            .when_some(parent, |d, (root, title)| {
                d.child(
                    Icon::new(IconName::ChevronRight)
                        .size(px(12.))
                        .text_color(m.text_faint),
                )
                .child(
                    div()
                        .id("album-parent")
                        .min_w_0()
                        .truncate()
                        .text_ellipsis_middle()
                        .cursor_pointer()
                        .child(title)
                        .on_click(move |_, w, cx| {
                            cx.stop_propagation();
                            album_browser
                                .update(cx, |b, cx| b.show(Location::Library(root), w, cx));
                        }),
                )
            })
            .when_some(release, |d, (key, title)| {
                d.child(
                    Icon::new(IconName::ChevronRight)
                        .size(px(12.))
                        .text_color(m.text_faint),
                )
                .child(
                    div()
                        .id("album-release")
                        .min_w_0()
                        .truncate()
                        .text_ellipsis_middle()
                        .cursor_pointer()
                        .child(title.clone())
                        .on_click(move |_, w, cx| {
                            cx.stop_propagation();
                            release_browser
                                .update(cx, |b, cx| b.open_release(&key, title.clone(), w, cx));
                        }),
                )
            })
            .when_some(name, |d, name| {
                d.child(
                    Icon::new(IconName::ChevronRight)
                        .size(px(12.))
                        .text_color(m.text_faint)
                        .mx(px(2.)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_ellipsis_middle()
                        .text_color(m.text)
                        .child(name),
                )
            })
            .into_any_element()
    }
    fn render_crumbs(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let m = *materials(cx);
        if !self.browser.read(cx).location.is_folder() {
            return self.render_view(cx);
        }
        let path = self.browser.read(cx).path.clone();
        let browser = self.browser.read(cx);
        let mut all = crumbs(&path);
        // A page names its collection's folder, then the release a CD
        // belongs to, then itself.
        if let Some(page) = &browser.page {
            if let Some(folder) = all.last_mut() {
                folder.step = Step::Collection;
            }
            if let (crate::collection::Page::Disc(_), Some((key, title))) =
                (page, browser.release())
            {
                all.push(Crumb {
                    label: title.clone().into(),
                    path: path.clone(),
                    step: Step::Release(key, title),
                });
            }
            all.push(Crumb {
                label: browser.crumb_title(cx).into(),
                path: path.clone(),
                step: Step::Current,
            });
        }
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
        let available = self.width.get() - px(28.);
        let widths: Vec<Pixels> = all.iter().map(|c| measure(&c.label) + pad).collect();
        let mut hidden = 0;
        if available > px(0.) {
            let mut total: Pixels = widths.iter().copied().sum::<Pixels>()
                + chevron * all.len().saturating_sub(1) as f32;
            while hidden + 1 < all.len() && total > available {
                total -= widths[hidden] + chevron;
                hidden += 1;
            }
            if hidden > 0 {
                total += px(28.);
                while hidden + 1 < all.len() && total > available {
                    total -= widths[hidden] + chevron;
                    hidden += 1;
                }
            }
        }
        let browser = self.browser.clone();
        let last = all.len().saturating_sub(1);
        let mut row = h_flex().h_full().min_w_0().items_center().overflow_hidden();
        if hidden > 0 {
            let collapsed: Vec<(SharedString, PathBuf, Step)> = all[..hidden]
                .iter()
                .rev()
                .map(|c| (c.label.clone(), c.path.clone(), c.step.clone()))
                .collect();
            let browser = browser.clone();
            row = row.child(
                div().occlude().child(
                    Button::new("crumbs-overflow")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Ellipsis).text_color(m.text_muted))
                        .accessibility_label("Earlier folders")
                        .dropdown_menu(move |mut menu, _, _| {
                            for (label, path, step) in &collapsed {
                                let step = step.clone();
                                let browser = browser.clone();
                                let path = path.clone();
                                menu = menu.item(PopupMenuItem::new(label.clone()).on_click(
                                    move |_, w, cx| {
                                        browser
                                            .update(cx, |b, cx| step.open(b, path.clone(), w, cx))
                                    },
                                ));
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
            let target = c.path.clone();
            let step = c.step.clone();
            row = row.child(
                div()
                    .id(("crumb", i))
                    .min_w_0()
                    .when(i != last, |d| d.flex_shrink_0())
                    .px(px(7.))
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .text_size(font_size)
                    .text_color(if i == last { m.text } else { m.text_muted })
                    .hover(|d| d.bg(m.hover).text_color(m.text))
                    .cursor_pointer()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_ellipsis_middle()
                            .child(c.label.clone()),
                    )
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(c.label.clone()).build(w, cx)
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, w, cx| {
                        cx.stop_propagation();
                        browser.update(cx, |b, cx| step.open(b, target.clone(), w, cx));
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
            .h(px(28.))
            .px(px(6.))
            .gap(px(4.))
            .rounded(px(6.))
            .border_1()
            .child(measure)
            .on_action(cx.listener(|bar, _: &CancelEdit, w, cx| bar.cancel(w, cx)));
        if self.editing {
            return base
                .bg(m.canvas)
                .border_color(if self.error.is_some() {
                    m.danger.opacity(0.7)
                } else {
                    m.accent.opacity(0.6)
                })
                .child(
                    Icon::new(IconName::FolderOpen)
                        .size(px(14.))
                        .text_color(m.text_faint)
                        .ml(px(3.)),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&self.input)
                            .appearance(false)
                            .small()
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
                            .max_w(px(320.))
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
        let copy = self.browser.read(cx).path.display().to_string();
        base.bg(m.hover)
            .border_color(gpui::transparent_black())
            .hover(|d| d.border_color(m.control_line))
            .cursor_text()
            .child(self.render_crumbs(window, cx))
            .child(div().flex_1().h_full())
            .on_click(cx.listener(|bar, _, w, cx| bar.edit(w, cx)))
            .context_menu(move |menu, _, _| {
                let copy = copy.clone();
                menu.item(
                    PopupMenuItem::new("Copy Address")
                        .icon(IconName::Copy)
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                        }),
                )
                .item(
                    PopupMenuItem::new("Edit Address")
                        .icon(IconName::Pencil)
                        .action(Box::new(crate::workspace::FocusLocation)),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{crumbs, parse};
    use std::path::{Path, PathBuf};
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
        let c = crumbs(Path::new("C:\\Users\\Music"));
        let labels: Vec<_> = c.iter().map(|c| c.label.to_string()).collect();
        assert_eq!(labels, ["C:", "Users", "Music"]);
        assert_eq!(c[0].path, PathBuf::from("C:\\"));
        let c = crumbs(Path::new("\\\\nas\\media\\Rips"));
        let labels: Vec<_> = c.iter().map(|c| c.label.to_string()).collect();
        assert_eq!(labels, ["\\\\nas\\media", "Rips"]);
        assert_eq!(c[1].path, PathBuf::from("\\\\nas\\media\\Rips"));
    }
}
