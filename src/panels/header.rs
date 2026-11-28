//! The header row shared by content panels such as the analyzers and the
//! Waveform deck: one height, inset and type scale, so headers of panels
//! that sit side by side line up and read alike.
//!
//! The dock tab names the panel, so a header never repeats that name. It
//! summarizes the panel's settings and holds the panel's own controls.
use crate::theme::{Materials, size};
use gpui_kit::{component::h_flex, prelude::*, *};

/// An empty header row. Callers add a [`summary`], [`detail`] and trailing
/// controls.
pub fn panel_header() -> Div {
    h_flex()
        .h(px(size::HEADER))
        .flex_shrink_0()
        .pl(px(size::INSET))
        .pr(px(size::INSET - 6.))
        .gap(px(size::GAP))
        .items_center()
}

/// What the panel shows, such as its view or measurement.
pub fn summary(label: impl Into<SharedString>, m: &Materials) -> Div {
    div()
        .flex_shrink_0()
        .text_size(px(size::TEXT_SMALL))
        .text_color(m.text)
        .child(label.into())
}

/// Its current settings or status, which gives way first when space is short.
pub fn detail(label: impl Into<SharedString>, m: &Materials) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_size(px(size::TEXT_SMALL))
        .text_color(m.text_muted)
        .child(label.into())
}
