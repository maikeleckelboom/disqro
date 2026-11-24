//! Rows shared by the navigation panels, and what they drag.
use crate::theme::{Materials, materials};
use gpui_kit::{
    assets::IconName,
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

/// The disclosure slot used by the Locations tree.
pub fn disclosure(id: impl Into<ElementId>, open: Option<bool>, m: &Materials) -> Stateful<Div> {
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

/// Every Library destination shares the same leading icon and label columns.
pub fn nav_row(
    id: ElementId,
    icon: Icon,
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
        .px(px(8.))
        .gap(px(4.))
        .rounded(px(5.))
        .flex_shrink_0()
        .items_center()
        .text_size(px(13.))
        .cursor_pointer()
        .text_color(if selected { m.text } else { m.text_muted })
        .when(selected, |d| d.bg(m.selected))
        .when(!selected, |d| d.hover(|d| d.bg(m.hover).text_color(m.text)))
        // Keyboard focus outlines the row inside a border it always reserves.
        .border_1()
        .border_color(transparent_black())
        .focus_visible(|d| d.text_color(m.text).border_color(m.focus_line()))
        .child(
            icon.size(px(16.))
                .flex_shrink_0()
                .mr(px(2.))
                .text_color(if selected { m.accent } else { m.text_faint }),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
}
