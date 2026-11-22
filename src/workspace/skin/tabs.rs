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
const READABLE_TAB: f32 = 96.;
const UNWRAP_GAP: f32 = 8.;

pub(super) struct TabLabels {
    pub all: Pixels,
    pub current: Pixels,
    pub several: bool,
}

pub(super) struct HeaderWidths {
    pub tabs: Pixels,
    pub controls: Pixels,
    pub overflow: bool,
}

impl TabLabels {
    fn identity(&self) -> Pixels {
        let switcher = if self.several { px(SWITCHER) } else { px(0.) };
        self.all.min(self.current.min(px(READABLE_TAB)) + switcher)
    }

    pub fn stacked(
        &self,
        width: Pixels,
        controls: Option<BrowserHeaderWidths>,
        was_stacked: bool,
    ) -> bool {
        controls.is_some_and(|controls| {
            let threshold = self.identity() + controls.minimum;
            width < threshold + if was_stacked { px(UNWRAP_GAP) } else { px(0.) }
        })
    }

    pub fn fit(
        &self,
        width: Pixels,
        controls: Option<BrowserHeaderWidths>,
        stacked: bool,
    ) -> HeaderWidths {
        let inline = controls.filter(|_| !stacked);
        // Allow the field to open into spare space. At the narrow end the
        // selected identity takes precedence and the controls can wrap.
        let budget = match inline {
            Some(controls) => (width - controls.editable).max(self.identity()).min(width),
            None => (width - px(crate::theme::size::TAB_DROP)).max(px(0.)),
        };
        let overflow = self.several && self.all > budget;
        let switcher = if overflow { px(SWITCHER) } else { px(0.) };
        let natural = if overflow { self.current } else { self.all };
        let tabs = natural.min((budget - switcher).max(px(0.)));
        HeaderWidths {
            tabs,
            controls: if stacked {
                width
            } else {
                (width - tabs - switcher).max(px(0.))
            },
            overflow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BrowserHeaderWidths, SWITCHER, TabLabels, px, tab_width};

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
        let needed = labels.all + collection(true).editable;
        assert!(!labels.fit(needed, Some(collection(true)), false).overflow);
        assert!(
            labels
                .fit(needed - px(1.), Some(collection(true)), false)
                .overflow
        );
    }

    fn collection(editing: bool) -> BrowserHeaderWidths {
        BrowserHeaderWidths {
            minimum: px(if editing { 216. } else { 140. }),
            editable: px(216.),
            full: px(if editing { 356. } else { 242. }),
        }
    }

    #[test]
    fn opening_filter_preserves_inline_tab_widths_and_overflow_boundary() {
        let labels = TabLabels {
            all: px(310.),
            current: px(110.),
            several: true,
        };
        for width in [525., 526., 1024.] {
            let closed = labels.fit(px(width), Some(collection(false)), false);
            let open = labels.fit(px(width), Some(collection(true)), false);
            assert_eq!(closed.overflow, width < 526.);
            assert_eq!(closed.overflow, open.overflow);
            assert_eq!(closed.tabs, open.tabs);
            assert_eq!(
                closed.tabs,
                if closed.overflow {
                    labels.current
                } else {
                    labels.all
                }
            );
            let switcher = if closed.overflow {
                px(SWITCHER)
            } else {
                px(0.)
            };
            assert_eq!(open.tabs + switcher + open.controls, px(width));
            assert!(open.controls >= collection(true).minimum);
        }
    }

    #[test]
    fn short_single_identity_wraps_only_when_essential_controls_need_the_space() {
        let labels = TabLabels {
            all: px(66.),
            current: px(66.),
            several: false,
        };
        let closed = Some(collection(false));
        let open = Some(collection(true));
        assert!(!labels.stacked(px(206.), closed, false));
        assert!(labels.stacked(px(206.), open, false));
        assert_eq!(labels.fit(px(206.), closed, false).tabs, px(66.));
        let second_row = labels.fit(px(206.), open, true);
        assert!(!second_row.overflow);
        assert_eq!(second_row.controls, px(206.));
        assert_eq!(second_row.tabs, px(66.));
    }

    #[test]
    fn resizing_around_wrap_boundary_settles_until_both_rows_fit_inline_comfortably() {
        let labels = TabLabels {
            all: px(400.),
            current: px(180.),
            several: true,
        };
        let controls = Some(collection(true));
        let mut stacked = false;
        for (width, expected) in [
            (338., false),
            (337., true),
            (338., true),
            (337., true),
            (345., true),
            (346., false),
            (338., false),
        ] {
            stacked = labels.stacked(px(width), controls, stacked);
            assert_eq!(stacked, expected, "group width {width}");
        }
    }

    #[test]
    fn utility_and_wrapped_identity_rows_retain_an_append_target() {
        let labels = TabLabels {
            all: px(180.),
            current: px(120.),
            several: true,
        };
        for width in [100., 187., 188., 720.] {
            assert!(!labels.stacked(px(width), None, true));
            let utility = labels.fit(px(width), None, false);
            let stacked = labels.fit(px(width), Some(collection(true)), true);
            assert_eq!(utility.overflow, width < 188.);
            assert_eq!(utility.tabs, stacked.tabs);
            let switcher = if utility.overflow {
                px(SWITCHER)
            } else {
                px(0.)
            };
            assert!(px(width) - utility.tabs - switcher >= px(crate::theme::size::TAB_DROP));
            assert_eq!(stacked.controls, px(width));
        }
    }
}
