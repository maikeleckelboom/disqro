//! The expand and collapse control shared by Library section headings, the
//! Locations tree and the Music Browser Tree. Each surface places it where
//! its composition needs it; its size, chevron, materials, accessibility and
//! event handling are the same everywhere.
use crate::theme::materials;
use gpui_kit::{IntoElement, assets::IconName, prelude::*, *};
use std::{cell::RefCell, collections::HashMap, hash::Hash, rc::Rc};

gpui_kit::actions!(disclosure, [ActivateDisclosure]);

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", ActivateDisclosure, Some("Disclosure")),
        KeyBinding::new("space", ActivateDisclosure, Some("Disclosure")),
    ]);
}

/// Kit's virtual lists give row wrappers positional IDs. Keep the focus handle
/// keyed by item across a reorder while the rendered control owns it. Dead
/// entries are collected after each group of new keys.
pub(crate) struct VirtualFocus<K> {
    handles: RefCell<HashMap<K, WeakFocusHandle>>,
    insertions: std::cell::Cell<usize>,
}

impl<K> Default for VirtualFocus<K> {
    fn default() -> Self {
        Self {
            handles: Default::default(),
            insertions: Default::default(),
        }
    }
}

impl<K: Eq + Hash> VirtualFocus<K> {
    pub fn handle(&self, key: K, cx: &App) -> FocusHandle {
        let mut handles = self.handles.borrow_mut();
        if let Some(handle) = handles.get(&key).and_then(WeakFocusHandle::upgrade) {
            return handle;
        }
        let count = self.insertions.get() + 1;
        if count == 128 {
            handles.retain(|_, handle| handle.upgrade().is_some());
            self.insertions.set(0);
        } else {
            self.insertions.set(count);
        }
        let handle = cx.focus_handle();
        handles.insert(key, handle.downgrade());
        handle
    }
}

/// The interactive area, in logical pixels.
pub const SIZE: f32 = 20.;
/// The chevron drawn at its center.
const CHEVRON: f32 = 12.;
const RADIUS: f32 = 4.;

type Toggle = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// A chevron that expands or collapses `subject` and does nothing else: its
/// press never reaches the row around it, so a row that navigates or
/// selects on click keeps doing so everywhere but here.
#[derive(IntoElement)]
pub struct Disclosure {
    id: ElementId,
    open: bool,
    subject: SharedString,
    disabled: bool,
    focus: Option<FocusHandle>,
    on_toggle: Option<Toggle>,
}

impl Disclosure {
    pub fn new(id: impl Into<ElementId>, open: bool, subject: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            open,
            subject: subject.into(),
            disabled: false,
            focus: None,
            on_toggle: None,
        }
    }

    /// Shows the state without accepting input, such as for a branch that
    /// is unavailable. The row around it shows why.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn track_focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
    }

    pub fn on_toggle(mut self, toggle: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(toggle));
        self
    }

    /// The empty space of an item that cannot expand, so its icon and label
    /// line up with those of items that can.
    pub fn slot() -> Div {
        div().size(px(SIZE)).flex_shrink_0()
    }
}

impl RenderOnce for Disclosure {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *materials(cx);
        let action = if self.open { "Collapse" } else { "Expand" };
        let interactive = !self.disabled && self.on_toggle.is_some();
        let group: SharedString = format!("disclosure-{:?}", self.id).into();
        div()
            .id(self.id)
            .group(group.clone())
            .role(Role::Button)
            .aria_label(format!("{action} {}", self.subject))
            .aria_expanded(self.open)
            .when(!interactive, |d| {
                d.a11y_synthetic_children(|tree| tree.parent_node().set_disabled())
            })
            .size(px(SIZE))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(RADIUS))
            // Keyboard focus outlines the control inside a border it always
            // reserves, so focus never moves the chevron.
            .border_1()
            .border_color(transparent_black())
            .when(interactive, |d| {
                d.key_context("Disclosure")
                    .focusable()
                    .when_some(self.focus, |d, focus| d.track_focus(&focus))
                    .tab_stop(true)
                    .hover(|d| d.bg(m.control))
                    .active(|d| d.bg(m.control_hover))
                    .focus_visible(|d| d.border_color(m.focus_line()))
            })
            .child(
                svg()
                    .path(
                        if self.open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        }
                        .path(),
                    )
                    .size(px(CHEVRON))
                    .flex_shrink_0()
                    .text_color(m.text_faint)
                    .when(interactive, |svg| {
                        svg.group_hover(group, |s| s.text_color(m.text_muted))
                    }),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when_some(self.on_toggle.filter(|_| interactive), |d, toggle| {
                let click = toggle.clone();
                d.on_action(move |_: &ActivateDisclosure, window, cx| toggle(window, cx))
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        click(window, cx);
                    })
            })
    }
}
