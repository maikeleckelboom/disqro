//! Rows shared by the navigation panels, and what they drag.
use crate::{
    disclosure::Disclosure,
    theme::{Materials, materials},
};
use gpui_kit::{
    component::{Icon, h_flex},
    prelude::*,
    *,
};
use std::path::PathBuf;

pub const ROW: f32 = 26.;
/// Each level of the Locations filesystem tree.
pub const INDENT: f32 = 14.;

/// A folder dragged from Locations: onto Library to add it there. Dragging
/// never moves or copies the folder itself.
#[derive(Clone)]
pub struct DraggedFolder {
    pub path: PathBuf,
    pub label: SharedString,
    /// The Place it is, so Locations can reorder Places by dragging.
    pub place: Option<usize>,
}
impl Render for DraggedFolder {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        chip(self.label.clone(), cx)
    }
}

/// The small label that follows the pointer while something is dragged.
pub fn chip(label: SharedString, cx: &App) -> Div {
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
        .child(label)
}

/// The leading icon column every navigation row shares.
pub const ICON: f32 = 16.;
/// Space between the leading icon column and the label.
const ICON_GAP: f32 = 2.;

/// The surface every navigation row shares: height, insets, hover,
/// selection and keyboard focus. Rows add their own leading column and label.
pub fn nav_surface(
    id: ElementId,
    label: SharedString,
    selected: bool,
    m: &Materials,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .tab_stop(true)
        .h(px(ROW))
        .mx(px(6.))
        .px(px(8.))
        .gap(px(4.))
        .rounded(px(5.))
        .flex_shrink_0()
        .items_center()
        .text_size(px(crate::theme::size::TEXT))
        .cursor_pointer()
        .text_color(if selected { m.text } else { m.text_muted })
        .when(selected, |d| d.bg(m.selected))
        .when(!selected, |d| d.hover(|d| d.bg(m.hover).text_color(m.text)))
        // Keyboard focus outlines the row inside a border it always reserves.
        .border_1()
        .border_color(transparent_black())
        .focus_visible(|d| d.text_color(m.text).border_color(m.focus_line()))
}

/// Every Library destination shares the same leading icon and label columns.
pub fn nav_row(
    id: ElementId,
    icon: Icon,
    label: SharedString,
    selected: bool,
    m: &Materials,
) -> Stateful<Div> {
    nav_surface(id, label.clone(), selected, m)
        .child(
            icon.size(px(ICON))
                .flex_shrink_0()
                .mr(px(ICON_GAP))
                .text_color(if selected { m.accent } else { m.text_faint }),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
}

/// A section heading in a navigation list: its label, then its disclosure
/// chevron, which only expands or collapses the section, then the rest of
/// the row, which opens the section's overview. The label starts where the
/// icons of the rows below it do, so the heading leads them without being
/// indented itself. Callers add trailing actions after it.
pub fn section_row(
    id: ElementId,
    title: SharedString,
    open: bool,
    selected: bool,
    m: &Materials,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    nav_surface(id, format!("Browse {title}").into(), selected, m)
        .key_context("LibrarySection")
        .aria_expanded(open)
        .font_weight(FontWeight::MEDIUM)
        .child(
            div()
                .flex_shrink(1.)
                .min_w_0()
                .truncate()
                .child(title.clone()),
        )
        .child(
            div()
                .flex_shrink_0()
                .ml(px(HEADING_DISCLOSURE_GAP))
                .child(Disclosure::new("disclosure", open, title).on_toggle(on_toggle)),
        )
        .child(div().flex_1())
}

/// The chevron's interactive area pads its glyph, so it overlaps the label
/// slightly to keep the glyph visibly attached to it.
const HEADING_DISCLOSURE_GAP: f32 = -5.;
