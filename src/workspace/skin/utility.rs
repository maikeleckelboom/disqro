use super::*;

// The xsmall menu button and the strip's trailing inset.
pub(super) const MENU_SPACE: f32 = 24. + 4.;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Presentation {
    Labels,
    Overflow,
}

fn fit(width: Pixels, labels: Pixels) -> Presentation {
    let available = width - px(MENU_SPACE);
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
                px(12.5),
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
            // Two text insets, matching the tab frame.
            (line.width.ceil() + px(2. * 8.)).clamp(px(48.), px(220.))
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
        .focus_visible(|d| d.bg(materials(cx).selected))
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
    use super::{MENU_SPACE, Presentation, fit, px};
    #[test]
    fn both_boundaries_reserve_the_menu_at_every_tab_count() {
        for count in 1..=4 {
            let labels = px(90. * count as f32);
            let full = labels + px(MENU_SPACE);
            assert_eq!(fit(full, labels), Presentation::Labels);
            assert_eq!(fit(full - px(0.5), labels), Presentation::Overflow);
        }
    }
}
