use super::*;

pub(super) fn tab_width(text: Pixels, utility: bool, snap: impl Fn(f32) -> Pixels) -> Pixels {
    let end = if utility {
        snap(TAB_X)
    } else {
        snap(TAB_ICON) + snap(TAB_CLOSE) + snap(TAB_GAP) * 2. + snap(TAB_END)
    };
    (text + snap(1.) * 2. + snap(TAB_X) + end).clamp(snap(40.), snap(220.))
}

/// Natural width includes both focus edges and the reserved close button.
pub(super) fn label_width(group: &TabGroupContext, ix: usize, window: &Window, cx: &App) -> Pixels {
    let panel = &group.panels()[ix];
    let text =
        crate::theme::label_width(tab_label(panel, cx), px(crate::theme::size::TEXT), window);
    let natural = tab_width(text, is_utility(panel, cx), |width| {
        window.pixel_snap(px(width))
    });
    device_ceil(natural, window.scale_factor())
}

/// The smallest whole number of device pixels that holds `width`. Layout
/// snaps a half device pixel toward zero, which would take it from the label.
fn device_ceil(width: Pixels, scale: f32) -> Pixels {
    px((f32::from(width) * scale - 1e-3).ceil() / scale)
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
pub(super) const SPACING: f32 = 2.;

pub(super) struct TabLabels {
    all: Pixels,
    single: Pixels,
    several: bool,
}

pub(super) struct StripWidths {
    pub tabs: Pixels,
    pub overflow: bool,
    pub switcher: Pixels,
}

impl TabLabels {
    pub fn new(tabs: &[(usize, Pixels)], gap: Pixels) -> Self {
        let (total, widest) = tabs
            .iter()
            .fold((px(0.), px(0.)), |(total, widest), &(_, width)| {
                (total + width, widest.max(width))
            });
        Self {
            all: total + gap * tabs.len().saturating_sub(1) as f32,
            single: widest,
            several: tabs.len() > 1,
        }
    }

    /// A strip holds only tabs. When they no longer fit beside an append
    /// target, the switcher stays in place as the visible tab changes.
    pub fn fit(&self, width: Pixels, snap: impl Fn(f32) -> Pixels) -> StripWidths {
        let budget = (width - snap(crate::theme::size::TAB_DROP)).max(px(0.));
        let all = snap(f32::from(self.all));
        let overflow = self.several && all > snap(f32::from(budget));
        let switcher = if overflow { snap(SWITCHER) } else { px(0.) };
        let natural = if overflow { self.single } else { all };
        StripWidths {
            tabs: natural.min((budget - switcher).max(px(0.))),
            overflow,
            switcher,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct InsertionSlot {
    ix: usize,
    x: Pixels,
}

pub(super) type TabBounds = Vec<(usize, Bounds<Pixels>)>;

/// Tab midpoints divide the drop area. Both halves bordering a gap and the
/// gap itself resolve to one slot, including when visible indices skip panels.
fn insertion_slot(
    tabs: &[(usize, Bounds<Pixels>)],
    count: usize,
    switcher: Pixels,
    x: Pixels,
) -> InsertionSlot {
    let mut previous_right = None;
    for &(ix, bounds) in tabs {
        if x < bounds.center().x {
            return InsertionSlot {
                ix,
                x: previous_right.map_or(bounds.left(), |right| (right + bounds.left()) * 0.5),
            };
        }
        previous_right = Some(bounds.right());
    }
    if let Some(&(ix, bounds)) = tabs.last()
        && x < bounds.right()
    {
        return InsertionSlot {
            ix: ix + 1,
            x: bounds.right(),
        };
    }
    InsertionSlot {
        ix: count,
        x: previous_right.unwrap_or_default() + switcher,
    }
}

pub(super) fn drop_targets(
    group: &TabGroupContext,
    state: &SkinState,
    bounds: Rc<RefCell<TabBounds>>,
    switcher: Pixels,
    cx: &App,
) -> Stateful<Div> {
    let group = group.clone();
    let count = group.panels().len();
    let ids: Rc<[_]> = group
        .panels()
        .iter()
        .map(|panel| panel.panel_id(cx))
        .collect();
    let slot =
        move |w: &Window| insertion_slot(&bounds.borrow(), count, switcher, w.mouse_position().x);
    let marker = Rc::new(Cell::new(None));
    let position = marker.clone();
    let color = materials(cx).accent;
    // This hitbox sits above the tab hitboxes and covers the entire strip.
    // It only handles dock payloads, leaving ordinary tab interactions intact.
    let targets = div().id("tab-insertion-target").absolute().inset_0().child(
        canvas(
            |_, _, _| (),
            move |bounds, _, w, _| {
                if let Some(x) = position.take() {
                    let width = w.pixel_snap(px(SPACING)).min(bounds.size.width);
                    let left = w
                        .pixel_snap(x - width * 0.5)
                        .clamp(bounds.left(), bounds.right() - width);
                    let inset = w.pixel_snap(px(3.));
                    w.paint_quad(fill(
                        Bounds::new(
                            point(left, bounds.top() + inset),
                            size(width, bounds.size.height - inset * 2.),
                        ),
                        color,
                    ));
                }
            },
        )
        .absolute()
        .inset_0(),
    );

    if group.is_droppable() {
        let preview = slot.clone();
        targets
            .drag_over::<DragPanel>(move |style, _, w, _| {
                marker.set(Some(preview(w).x));
                style
            })
            .on_drop(move |drag: &DragPanel, w, cx| {
                let mut ix = slot(w).ix;
                // The dock API indexes the group after removing the source tab.
                if drag.source() == group.node() && ids[..ix].contains(&drag.panel()) {
                    ix -= 1;
                }
                group.drop_panel(drag.clone(), Some(ix), true, w, cx);
            })
    } else {
        let target = move |ix: usize| match ids.get(ix) {
            Some(id) => (*id, TabDrop::Before),
            None => (ids[count - 1], TabDrop::After),
        };
        let preview = slot.clone();
        let preview_target = target.clone();
        let ws = state.workspace.clone();
        let owner = ws.clone();
        targets
            .drag_over::<RoutineTab>(move |style, drag, w, cx| {
                let slot = preview(w);
                let (id, edge) = preview_target(slot.ix);
                if ws
                    .upgrade()
                    .is_some_and(|ws| ws.read(cx).routine_move(drag.0, id, edge, cx).is_some())
                {
                    marker.set(Some(slot.x));
                }
                style
            })
            .on_drop(move |drag: &RoutineTab, w, cx| {
                let (id, edge) = target(slot(w).ix);
                let _ = owner.update(cx, |ws, cx| ws.drop_routine_tab(drag.0, id, edge, w, cx));
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InsertionSlot, SPACING, SWITCHER, TabLabels, device_ceil, insertion_slot, tab_width,
    };
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn music_tab_fits_its_name_and_close_slot_at_the_last_inline_pixel() {
        let music = tab_width(px(35.), false, px);
        // Text plus focus edges, insets, folder icon, two gaps and close target.
        assert_eq!(music, px(105.));
        let library = tab_width(px(35.), true, px);
        assert_eq!(library, px(61.));
        let labels = TabLabels::new(&[(0, music), (2, library)], px(SPACING));
        let needed = music + library + px(SPACING + crate::theme::size::TAB_DROP);
        assert!(!labels.fit(needed, px).overflow);
        assert!(labels.fit(needed - px(1.), px).overflow);
    }

    #[test]
    fn natural_tab_widths_never_lose_a_device_pixel_to_layout_snapping() {
        // At 150 % the Library tab needed 60⅓ logical pixels: 90.5 device
        // pixels, which layout rounded down to 90 and truncated the label.
        assert_eq!(device_ceil(px(60. + 1. / 3.), 1.5), px(91. / 1.5));
        for scale in [1., 1.25, 1.5, 1.75, 2.] {
            for width in [57., 60.5, 61., 95., 120.25] {
                let snapped = device_ceil(px(width), scale);
                let device = f32::from(snapped) * scale;
                assert!(snapped >= px(width - 1e-3));
                assert!((device - device.round()).abs() < 1e-3);
                assert!(f32::from(snapped) - width < 1. / scale);
            }
        }
    }

    #[test]
    fn every_strip_retains_an_append_target() {
        let labels = TabLabels::new(&[(0, px(120.)), (1, px(58.))], px(SPACING));
        for width in [100., 187., 188., 720.] {
            let strip = labels.fit(px(width), px);
            assert_eq!(strip.overflow, width < 188.);
            let switcher = if strip.overflow { px(SWITCHER) } else { px(0.) };
            assert!(px(width) - strip.tabs - switcher >= px(crate::theme::size::TAB_DROP));
            if !strip.overflow {
                assert_eq!(strip.tabs, labels.all);
            }
        }
    }

    #[test]
    fn only_visible_tabs_contribute_gaps() {
        for (widths, total) in [
            (&[(2, px(60.))][..], 60.),
            (&[(0, px(60.)), (2, px(40.)), (5, px(50.))][..], 154.),
        ] {
            let labels = TabLabels::new(widths, px(SPACING));
            assert_eq!(labels.fit(px(200.), px).tabs, px(total));
        }
    }

    #[test]
    fn gap_and_adjacent_halves_share_one_drop_slot() {
        let rect = |x, w| Bounds::new(point(px(x), px(0.)), size(px(w), px(28.)));
        let tabs = [
            (0, rect(20., 60.)),
            (2, rect(82., 40.)),
            (3, rect(124., 50.)),
        ];
        for (x, ix, marker) in [
            (20., 0, 20.),
            (60., 2, 81.),
            (81., 2, 81.),
            (83., 2, 81.),
            (123., 3, 123.),
            (150., 4, 174.),
            (200., 4, 174.),
        ] {
            assert_eq!(
                insertion_slot(&tabs, 4, px(0.), px(x)),
                InsertionSlot { ix, x: px(marker) }
            );
        }
    }

    #[test]
    fn overflow_distinguishes_the_visible_tabs_trailing_half_from_append() {
        let bounds = Bounds::new(point(px(20.), px(0.)), size(px(60.), px(28.)));
        let tabs = [(1, bounds)];
        assert_eq!(
            insertion_slot(&tabs, 4, px(SWITCHER), px(60.)),
            InsertionSlot { ix: 2, x: px(80.) }
        );
        assert_eq!(
            insertion_slot(&tabs, 4, px(SWITCHER), px(112.)),
            InsertionSlot { ix: 4, x: px(106.) }
        );
    }
}
