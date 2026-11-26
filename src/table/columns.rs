//! Column and viewport behavior shared by the browser's virtualized tables.
use super::layout::Pin;
use super::*;
use gpui_kit::component::{FocusableExt as _, Size, table::DataTable};

#[derive(Default)]
pub(crate) struct ColumnInteraction {
    pub(super) viewport: f32,
    horizontal_scroll: Option<VirtualListScrollHandle>,
    resizing: Option<Resize>,
}

impl ColumnInteraction {
    pub(super) fn is_resizing(&self) -> bool {
        self.resizing.is_some()
    }
}

pub(crate) fn managed_table<D: TableDelegate>(
    delegate: D,
    window: &mut Window,
    cx: &mut Context<TableState<D>>,
) -> TableState<D> {
    TableState::new(delegate, window, cx)
        .col_selectable(false)
        .col_resizable(false)
        .sortable(false)
}

pub(crate) fn browser_table<D: TableDelegate>(
    state: &Entity<TableState<D>>,
    height: f32,
    label: &'static str,
) -> DataTable<D> {
    DataTable::new(state)
        .accessibility_label(label)
        .focus_ring(false)
        .fixed_cols_border(false)
        .bordered(false)
        .stripe(false)
        .header_row_height(px(crate::theme::size::HEADER))
        .with_size(Size::Size(px(height)))
}

pub(crate) fn fit_columns<D: TableColumns>(
    table: &Entity<TableState<D>>,
    width: f32,
    window: &mut Window,
    cx: &mut App,
) {
    let changed = table.update(cx, |t, cx| {
        if t.delegate().interaction().viewport == width {
            return false;
        }
        let scroll = t.horizontal_scroll_handle.clone();
        let columns = t.delegate_mut().interaction_mut();
        columns.horizontal_scroll = Some(scroll);
        columns.viewport = width;
        t.refresh(cx);
        cx.notify();
        true
    });
    if changed {
        let table = table.clone();
        window.on_next_frame(move |_, cx| {
            table.update(cx, |t, cx| {
                t.refresh(cx);
                cx.notify();
            });
        });
    }
}

pub(crate) trait TableColumns: TableDelegate {
    fn layout(&self) -> &Layout;
    fn layout_mut(&mut self) -> &mut Layout;
    fn interaction(&self) -> &ColumnInteraction;
    fn interaction_mut(&mut self) -> &mut ColumnInteraction;
    fn default_layout(&self) -> Layout;
    fn browser(&self) -> Option<WeakEntity<crate::browser::Browser>>;
    fn sort(&self) -> Option<(Field, ColumnSort)>;
    fn sort_rows(table: &mut TableState<Self>, field: Field, cx: &mut Context<TableState<Self>>);
    fn clear_sort_if_hidden(table: &mut TableState<Self>, cx: &mut Context<TableState<Self>>);

    fn available_fields(&self) -> &'static [Field] {
        &Field::ALL
    }
    fn column_available(&self, _: Field) -> bool {
        true
    }
    fn sortable_field(&self, field: Field) -> bool {
        field.sortable()
    }
    fn column_label(&self, field: Field) -> &'static str {
        field.label()
    }
    fn column_heading(&self, field: Field) -> &'static str {
        field.heading()
    }

    fn save_layout(&self, cx: &mut App) {
        if let Some(browser) = self.browser() {
            let _ = browser.update(cx, |_, cx| cx.emit(crate::browser::ViewChanged));
        }
    }
    fn table_column(&self, ix: usize) -> Column {
        let c = &self.layout().columns[ix];
        let pad = c.field.padding();
        let widths = self.layout().widths(self.interaction().viewport);
        let scrolling = self
            .layout()
            .scrolling(&widths, self.interaction().viewport);
        let mut col = Column::new(c.field.label(), self.column_label(c.field))
            .width(widths[ix])
            .min_width(px(c.field.min_width()))
            .max_width(px(c.field.max_width()))
            .paddings(Edges {
                top: px(0.),
                bottom: px(0.),
                left: px(pad),
                right: px(pad),
            });
        if ix < scrolling.start {
            col = col.fixed_left();
        } else if ix >= scrolling.end {
            col = col.fixed_right();
        }
        if c.field.right_aligned() {
            col = col.text_right();
        }
        col
    }
    fn move_table_column(
        &mut self,
        ix: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        if !self.layout_mut().move_column(ix, to) {
            // Kit moves its own groups after calling the delegate. Undo a drop
            // across a pin temporarily released in a narrow pane.
            cx.defer_in(window, |table, _, cx| table.refresh(cx));
        }
    }
    fn table_heading(
        &mut self,
        col_ix: usize,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let field = self.layout().columns[col_ix].field;
        let m = *materials(cx);
        let sorted = self.sort().filter(|(f, _)| *f == field).map(|(_, d)| d);
        let layout = self.layout();
        let pin = layout.columns[col_ix].pin;
        let widths = layout.widths(self.interaction().viewport);
        let scrolling = layout.scrolling(&widths, self.interaction().viewport);
        let offset = self
            .interaction()
            .horizontal_scroll
            .as_ref()
            .map_or(0., |s| f32::from(s.offset().x));
        let grip = |leading, cx: &mut Context<TableState<Self>>| {
            resize_owner(
                &widths,
                &scrolling,
                self.interaction().viewport,
                offset,
                col_ix,
                leading,
            )
            .map(|owner| self.render_edge(owner, col_ix, leading, owner >= scrolling.end, cx))
        };
        let leading = grip(true, cx);
        let trailing = grip(false, cx);
        // Artwork explains itself. Its heading keeps the name for assistive
        // technology and the column menu, without printing it.
        sort_heading(
            col_ix,
            self.column_heading(field),
            field.right_aligned(),
            sorted,
            cx,
        )
        .aria_label(self.column_label(field))
        .when(field == Field::Favorite, |d| d.justify_center())
        .children(trailing)
        .children(leading)
        .when(self.sortable_field(field), |d| {
            d.hover(|d| d.text_color(m.text_muted))
                .cursor_pointer()
                .on_click(cx.listener(move |t, _, w, cx| {
                    Self::sort_rows(t, field, cx);
                    t.refresh(cx);
                    cx.notify();
                    if let Some(browser) = t.delegate().browser() {
                        cx.defer_in(w, move |_, _, cx| {
                            let _ =
                                browser.update(cx, |_, cx| cx.emit(crate::browser::ViewChanged));
                        });
                    }
                }))
        })
        .context_menu({
            let table = cx.entity().downgrade();
            let fields: Vec<_> = self
                .available_fields()
                .iter()
                .copied()
                .filter(|field| self.column_available(*field))
                .collect();
            let shown: Vec<Field> = self.layout().columns.iter().map(|c| c.field).collect();
            move |mut menu, _, _| {
                enum Edit {
                    Pin(Pin),
                    Toggle(Field),
                    Reset,
                }
                let edit = |action: Edit| {
                    let table = table.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let _ = table.update(cx, |t, cx| {
                            let d = t.delegate_mut();
                            match action {
                                Edit::Pin(pin) => d.layout_mut().set_pin(field, pin),
                                Edit::Toggle(field) => d.layout_mut().toggle(field),
                                Edit::Reset => {
                                    *d.layout_mut() = d.default_layout();
                                }
                            }
                            Self::clear_sort_if_hidden(t, cx);
                            t.refresh(cx);
                            t.delegate().save_layout(cx);
                            cx.notify();
                        });
                    }
                };
                for (side, label) in [
                    (Pin::Left, "Pin to left"),
                    (Pin::Right, "Pin to right"),
                    (Pin::None, "Unpin"),
                ] {
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .checked(pin == side)
                            .on_click(edit(Edit::Pin(side))),
                    );
                }
                menu = menu.separator().label("Columns");
                for &f in &fields {
                    menu = menu.item(
                        PopupMenuItem::new(f.menu_label())
                            .checked(shown.contains(&f))
                            .disabled(f == Field::Title)
                            .on_click(edit(Edit::Toggle(f))),
                    );
                }
                menu.separator()
                    .item(PopupMenuItem::new("Reset Columns").on_click(edit(Edit::Reset)))
            }
        })
    }
    fn table_tail(&self) -> Div {
        // Leave just enough travel to expose the last scrolling divider
        // separately from the first right pin. The space has no hit target.
        let layout = self.layout();
        let widths = layout.widths(self.interaction().viewport);
        let range = layout.scrolling(&widths, self.interaction().viewport);
        div()
            .flex_shrink_0()
            .w(px(scroll_tail(&range, widths.len())))
    }
    /// Follows a dragged column boundary, updating the table on every move.
    /// The first move fixes the other columns at their shown widths, so only
    /// this boundary moves and the table scrolls rather than squeezing.
    fn drag_edge(table: &mut TableState<Self>, x: Pixels, cx: &mut Context<TableState<Self>>) {
        let Some(r) = table.delegate().interaction().resizing else {
            return;
        };
        if table.delegate().layout().adaptive {
            let shown: Vec<f32> = (0..table.delegate().layout().columns.len())
                .map_while(|ix| table.col_width(ix).map(f32::from))
                .collect();
            table.delegate_mut().layout_mut().freeze(&shown);
        }
        let Some(column) = table.delegate_mut().layout_mut().columns.get_mut(r.ix) else {
            return;
        };
        let field = column.field;
        let delta = if r.grow_left { r.x - x } else { x - r.x };
        let width = (r.width + f32::from(delta))
            .clamp(field.min_width(), field.max_width())
            .round();
        if column.width != width {
            column.width = width;
            table.refresh(cx);
            if let Some(gap) = r.end_gap {
                let d = table.delegate();
                let widths = d.layout().widths(d.interaction().viewport);
                let x = px((d.interaction().viewport - widths.iter().sum::<f32>() - gap).min(0.));
                table.horizontal_scroll_handle.set_offset(point(x, px(0.)));
            }
            cx.notify();
        }
    }
    fn end_resize(table: &mut TableState<Self>, cx: &mut Context<TableState<Self>>) {
        if table
            .delegate_mut()
            .interaction_mut()
            .resizing
            .take()
            .is_some()
        {
            let d = table.delegate();
            let shown = d.layout().widths(d.interaction().viewport);
            table.delegate_mut().layout_mut().settle(&shown);
            table.refresh(cx);
            table.delegate().save_layout(cx);
            cx.notify();
        }
    }
    /// Each half of a resize grip sits inside its host header. Right-fixed
    /// columns grow from their left edge, toward the scrolling area.
    fn render_edge(
        &self,
        ix: usize,
        host: usize,
        leading: bool,
        grow_left: bool,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let m = *materials(cx);
        let pad = -px(self.layout().columns[host].field.padding());
        let active = self.interaction().resizing.is_some_and(|r| r.ix == ix);
        div()
            .id((if leading { "edge-leading" } else { "edge" }, host))
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(EDGE))
            .occlude()
            .cursor_col_resize()
            .map(|d| {
                if leading {
                    d.left(pad).border_l_1()
                } else {
                    d.right(pad).border_r_1()
                }
            })
            .border_color(if active && ix == host {
                m.text_faint
            } else {
                transparent_black()
            })
            .when(!active, |d| d.hover(|d| d.border_color(m.control_line)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |t, e: &MouseDownEvent, _, _| {
                    let d = t.delegate();
                    let widths = d.layout().widths(d.interaction().viewport);
                    let scrolling = d.layout().scrolling(&widths, d.interaction().viewport);
                    // Keep the last visible scrollable edge in view, including
                    // when it is flush with the first right pin before the drag.
                    let offset = f32::from(t.horizontal_scroll_handle.offset().x);
                    let gap = d.interaction().viewport - widths.iter().sum::<f32>() - offset;
                    let end_gap =
                        (scrolling.end < widths.len() && ix + 1 >= scrolling.end && gap >= -1.)
                            .then(|| gap.clamp(0., scroll_tail(&scrolling, widths.len())));
                    let width = t.col_width(ix).map(f32::from);
                    t.delegate_mut().layout_mut().holding = width.map(|_| ix);
                    t.delegate_mut().interaction_mut().resizing = width.map(|width| Resize {
                        ix,
                        x: e.position.x,
                        width,
                        grow_left,
                        end_gap,
                    });
                }),
            )
            .on_drag(ColumnEdge, |edge, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| edge.clone())
            })
            .on_drag_move(cx.listener(|t, e: &DragMoveEvent<ColumnEdge>, _, cx| {
                Self::drag_edge(t, e.event.position.x, cx)
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|t, _, _, cx| Self::end_resize(t, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|t, _, _, cx| Self::end_resize(t, cx)),
            )
    }
}

/// Grab room on each side of a column boundary in the table's
/// heading. Both halves sit in the cells' padding.
const EDGE: f32 = 6.;
pub(super) fn scroll_tail(scrolling: &Range<usize>, count: usize) -> f32 {
    if !scrolling.is_empty() && scrolling.end < count {
        2. * EDGE
    } else {
        0.
    }
}

/// Resolve a header's half of a divider in viewport coordinates. At a pin
/// seam both sides address the fixed column. A clipped edge has no handle.
pub(super) fn resize_owner(
    widths: &[f32],
    scrolling: &Range<usize>,
    viewport: f32,
    offset: f32,
    host: usize,
    leading: bool,
) -> Option<usize> {
    let boundary = if leading { host } else { host + 1 };
    if host >= scrolling.end {
        return (boundary < widths.len()).then_some(boundary);
    }
    let left: f32 = widths[..scrolling.start].iter().sum();
    let right = viewport - widths[scrolling.end..].iter().sum::<f32>();
    let mut x = widths[..boundary].iter().sum::<f32>();
    if scrolling.contains(&host) {
        x += offset;
        if x < left - 0.5 || x > right + 0.5 {
            return None;
        }
        if scrolling.start > 0 && (x - left).abs() <= 0.5 {
            return Some(scrolling.start - 1);
        }
    }
    if scrolling.end < widths.len() && (x - right).abs() <= 0.5 {
        return Some(scrolling.end);
    }
    boundary.checked_sub(1)
}

/// A column boundary dragged in the table's heading.
#[derive(Clone)]
struct ColumnEdge;
impl Render for ColumnEdge {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
/// The column being resized: where the press began, and its shown width then.
#[derive(Clone, Copy)]
struct Resize {
    ix: usize,
    x: Pixels,
    width: f32,
    grow_left: bool,
    end_gap: Option<f32>,
}
