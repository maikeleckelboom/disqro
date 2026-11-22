use super::*;

/// Natural tab widths, including the icon on content panels and both
/// reserved focus edges. The same limits are applied by the tab renderer.
pub(super) fn labels_width(
    group: &TabGroupContext,
    tabs: &[usize],
    window: &Window,
    cx: &App,
) -> Pixels {
    let font = window.text_style().font();
    tabs.iter()
        .map(|&ix| {
            let panel = &group.panels()[ix];
            let label = tab_label(panel, cx);
            let icon = if is_utility(panel, cx) { 0. } else { 20. };
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
            (line.width.ceil() + px(2. * (TAB_X + 1.) + icon)).clamp(px(40.), px(220.))
        })
        .sum()
}

pub(super) fn keyboard_tab(
    tab: Stateful<Div>,
    group: &TabGroupContext,
    ix: usize,
    selected: bool,
    cx: &App,
) -> Stateful<Div> {
    let Some(pane) = pane_of(&group.panels()[ix]) else {
        return tab;
    };
    let group = group.clone();
    tab.track_focus(&pane.read(cx).tab_focus)
        .tab_index(0)
        .tab_stop(selected)
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

/// Fit all the labels when possible. In overflow, reserve just the selected
/// label and a switcher. Context controls get the remaining group width.
pub(super) struct HeaderWidths {
    pub tabs: Pixels,
    pub controls: Pixels,
    pub overflow: bool,
}

impl HeaderWidths {
    pub fn fit(
        width: Pixels,
        minimum_controls: Pixels,
        labels: Pixels,
        current: Pixels,
        several: bool,
        stacked: bool,
    ) -> Self {
        let available = (width - px(4.)).max(px(0.));
        let budget = if stacked {
            available
        } else {
            available - minimum_controls
        };
        let overflow = several && labels > budget;
        let switcher = if overflow {
            px(28.).min(available)
        } else {
            px(0.)
        };
        let tabs = (if overflow { current } else { labels }).min((budget - switcher).max(px(0.)));
        let controls = if stacked {
            available
        } else {
            available - tabs - switcher
        };
        Self {
            tabs,
            controls,
            overflow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HeaderWidths, px};
    #[test]
    fn tabs_fit_to_the_control_budget_of_their_own_group() {
        let fit = |width, stacked| {
            HeaderWidths::fit(px(width), px(140.), px(260.), px(152.), true, stacked)
        };
        assert!(!fit(404., false).overflow);
        let narrow = fit(403., false);
        assert!(narrow.overflow);
        assert_eq!(narrow.tabs, px(152.));
        assert!(narrow.controls >= px(140.));
        assert!(!fit(276., true).overflow);
        assert_eq!(fit(276., true).controls, px(272.));
        let utility = HeaderWidths::fit(px(90.), px(0.), px(170.), px(76.), true, false);
        assert!(utility.overflow);
        assert_eq!(utility.tabs, px(58.));
    }
}
