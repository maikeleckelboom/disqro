use super::*;

// The strip's insets on both sides.
pub(super) const STRIP_INSETS: f32 = 2. * 4.;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Presentation {
    Labels,
    Overflow,
}

fn fit(width: Pixels, labels: Pixels) -> Presentation {
    let available = width - px(STRIP_INSETS);
    if available >= labels {
        Presentation::Labels
    } else {
        Presentation::Overflow
    }
}

pub(super) fn presentation(
    group: &TabGroupContext,
    tabs: &[usize],
    width: Pixels,
    window: &mut Window,
    cx: &App,
) -> Presentation {
    let font = window.text_style().font();
    let labels = tabs
        .iter()
        .filter_map(|&ix| pane_of(&group.panels()[ix]))
        .map(|pane| {
            let label = pane.read(cx).label(cx);
            let line = window.text_system().shape_line(
                label.clone(),
                px(crate::theme::size::TEXT),
                &[TextRun {
                    len: label.len(),
                    font: font.clone(),
                    color: materials(cx).text,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            // Two text insets and the focus border, matching the tab frame.
            (line.width.ceil() + px(2. * (TAB_X + 1.))).clamp(px(40.), px(220.))
        })
        .sum();
    fit(width, labels)
}

pub(super) fn keyboard_tab(
    tab: Stateful<Div>,
    group: &TabGroupContext,
    ix: usize,
    cx: &App,
) -> Stateful<Div> {
    let Some(pane) = pane_of(&group.panels()[ix]) else {
        return tab;
    };
    let group = group.clone();
    tab.track_focus(&pane.read(cx).tab_focus)
        .tab_index(0)
        .tab_stop(ix == group.active_ix())
        .focus_visible(|d| d.border_color(materials(cx).focus_line()))
        .on_key_down(move |event, window, cx| {
            if event.keystroke.modifiers.modified() {
                return;
            }
            let count = group.panels().len();
            let direction = match event.keystroke.key.as_str() {
                "left" => count - 1,
                "right" => 1,
                _ => return,
            };
            if let Some(next) = (1..=count)
                .map(|n| (ix + direction * n) % count)
                .find(|&i| group.panels()[i].visible(cx))
                && let Some(target) = pane_of(&group.panels()[next])
            {
                group.select_tab(next, window, cx);
                target.read(cx).tab_focus.clone().focus(window, cx);
                cx.stop_propagation();
            }
        })
}

#[cfg(test)]
mod tests {
    use super::{Presentation, STRIP_INSETS, fit, px};
    #[test]
    fn labels_show_exactly_while_they_fit_at_every_tab_count() {
        for count in 1..=4 {
            let labels = px(90. * count as f32);
            let full = labels + px(STRIP_INSETS);
            assert_eq!(fit(full, labels), Presentation::Labels);
            assert_eq!(fit(full - px(0.5), labels), Presentation::Overflow);
        }
    }
}
