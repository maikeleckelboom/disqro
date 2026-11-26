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

/// A section heading in a navigation list. Its disclosure chevron sits in the
/// leading icon column and only expands or collapses the section. The rest of
/// the row opens the section's overview.
pub fn section_row(
    id: ElementId,
    title: SharedString,
    open: bool,
    selected: bool,
    m: &Materials,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let action = if open { "Collapse" } else { "Expand" };
    nav_surface(id, format!("Browse {title}").into(), selected, m)
        .aria_expanded(open)
        .font_weight(FontWeight::MEDIUM)
        .child(
            div()
                .id("disclosure")
                .role(Role::Button)
                .aria_label(format!("{action} {title}"))
                .aria_expanded(open)
                .size(px(ICON))
                .mr(px(ICON_GAP))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .hover(|d| d.bg(m.control_hover))
                .child(
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(px(12.))
                    .text_color(m.text_faint),
                )
                // The heading opens its overview on click. The chevron must not.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    on_toggle(window, cx);
                }),
        )
        .child(div().flex_1().min_w_0().truncate().child(title))
}
