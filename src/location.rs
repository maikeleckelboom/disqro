//! Browser history controls and the explicit Go to Folder or File editor.
use crate::{
    browser::{Browser, Location, Reveal},
    theme::materials,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        Icon, Sizable, h_flex,
        input::{Input, InputEvent, InputState},
        tooltip::Tooltip,
    },
    prelude::*,
    *,
};
use std::path::{Path, PathBuf};

gpui_kit::actions!(location, [CancelEdit]);

/// Height shared by the shell editor and the contextual browser filter.
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
    _view: Subscription,
}

/// Only a physical folder view has a filesystem address. A CD page can be
/// projected from a folder while its navigation identity is the CD itself.
pub fn physical_address<'a>(
    location: &'a Location,
    page: Option<&crate::collection::Page>,
) -> Option<&'a Path> {
    match (location, page) {
        (Location::Folder(path), None) => Some(path),
        _ => None,
    }
}

/// Turns what the user typed or pasted into an absolute path, or explains
/// why it is not one.
pub fn parse(raw: &str) -> Result<PathBuf, &'static str> {
    let mut s = raw.trim().trim_matches('"').trim().to_owned();
    if s.is_empty() {
        return Err("Enter a folder or file path");
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
        Ok(_) => Target::Unavailable("Choose a folder, an audio file or a CUE sheet".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Target::Missing,
        Err(e) => Target::Unavailable(e.to_string()),
    }
}

impl LocationBar {
    pub fn new(browser: Entity<Browser>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Enter a folder or file path…"));
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
            _view: view,
        }
    }
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }
    fn follow(
        browser: &Entity<Browser>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(
            browser,
            window,
            |bar, browser, _: &crate::browser::ViewChanged, window, cx| {
                let browser = browser.read(cx);
                // Metadata and table updates also emit ViewChanged. Keep a draft
                // while its browser still shows the destination it was opened from.
                if bar.editing.as_ref().is_some_and(|(location, page)| {
                    *location != browser.location || *page != browser.page
                }) {
                    bar.cancel(window, cx);
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
        let was_focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        self.reset(cx);
        if was_focused {
            self.focus_list(window, cx);
        }
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
                if !bar.browser.read(cx).active {
                    bar.reset(cx);
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
    fn editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *materials(cx);
        h_flex()
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
            .on_action(cx.listener(|bar, _: &CancelEdit, w, cx| bar.cancel(w, cx)))
            .capture_key_down(cx.listener(|bar, e: &KeyDownEvent, w, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    bar.cancel(w, cx);
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }))
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
                        .aria_label("Enter a folder or file path"),
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
                        .id("location-error")
                        .role(Role::Alert)
                        .aria_label(e.clone())
                        .flex_shrink_0()
                        .max_w(relative(0.5))
                        .gap_1()
                        .pr(px(6.))
                        .text_size(px(12.))
                        .text_color(m.danger)
                        .child(Icon::new(IconName::CircleAlert).size(px(13.)))
                        .child(div().truncate().text_ellipsis_middle().child(e.clone()))
                        .tooltip(move |w, cx| Tooltip::new(e.clone()).build(w, cx)),
                )
            })
    }
}
impl Render for LocationBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The browser header shows the editor in place of its location while
        // a path is being entered.
        if self.editing.is_some() {
            self.editor(cx).into_any_element()
        } else {
            Empty.into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
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
}
