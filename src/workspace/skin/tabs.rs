use super::*;

pub(super) fn tab_width(text: Pixels, utility: bool) -> Pixels {
    let end = if utility {
        TAB_X
    } else {
        TAB_ICON + TAB_CLOSE + 2. * TAB_GAP + TAB_END
    };
    (text + px(2. + TAB_X + end)).clamp(px(40.), px(220.))
}

/// Natural widths include both focus edges and the in-flow hover target.
pub(super) fn labels_width(
    group: &TabGroupContext,
    tabs: &[usize],
    window: &Window,
    cx: &App,
) -> Pixels {
    tabs.iter()
        .map(|&ix| {
            let panel = &group.panels()[ix];
            let label = tab_label(panel, cx);
            let text = crate::theme::label_width(label, px(crate::theme::size::TEXT), window);
            tab_width(text, is_utility(panel, cx))
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

/// The switcher has the same width in the fit calculation and the rendered strip.
pub(super) const SWITCHER: f32 = 26.;

pub(super) struct TabLabels {
    pub all: Pixels,
    pub current: Pixels,
    pub several: bool,
}

pub(super) struct StripWidths {
    pub tabs: Pixels,
    pub overflow: bool,
}

impl TabLabels {
    /// A strip holds only tabs. When they no longer fit beside an append
    /// target, it shows the selected tab and a switcher for the rest.
    pub fn fit(&self, width: Pixels) -> StripWidths {
        let budget = (width - px(crate::theme::size::TAB_DROP)).max(px(0.));
        let overflow = self.several && self.all > budget;
        let switcher = if overflow { px(SWITCHER) } else { px(0.) };
        let natural = if overflow { self.current } else { self.all };
        StripWidths {
            tabs: natural.min((budget - switcher).max(px(0.))),
            overflow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SWITCHER, TabLabels, px, tab_width};

    #[test]
    fn music_tab_fits_its_name_and_close_slot_at_the_last_inline_pixel() {
        let music = tab_width(px(35.), false);
        // Text plus focus edges, insets, folder icon, two gaps and close target.
        assert_eq!(music, px(95.));
        let library = tab_width(px(35.), true);
        assert_eq!(library, px(57.));
        let labels = TabLabels {
            current: music,
            all: music + library,
            several: true,
        };
        let needed = labels.all + px(crate::theme::size::TAB_DROP);
        assert!(!labels.fit(needed).overflow);
        assert!(labels.fit(needed - px(1.)).overflow);
    }

    #[test]
    fn every_strip_retains_an_append_target() {
        let labels = TabLabels {
            all: px(180.),
            current: px(120.),
            several: true,
        };
        for width in [100., 187., 188., 720.] {
            let strip = labels.fit(px(width));
            assert_eq!(strip.overflow, width < 188.);
            let switcher = if strip.overflow { px(SWITCHER) } else { px(0.) };
            assert!(px(width) - strip.tabs - switcher >= px(crate::theme::size::TAB_DROP));
            if !strip.overflow {
                assert_eq!(strip.tabs, labels.all);
            }
        }
    }
}
