use super::*;
use crate::{
    panels::Router,
    table::{
        ColumnInteraction, ContextKind, Layout, TREE_FIELDS, TableColumns, next_sort,
        stable_element_key,
    },
    theme::materials,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, h_flex,
        menu::{PopupMenu, PopupMenuItem},
        table::{Column, TableDelegate},
    },
    prelude::*,
};
use std::ops::Range;

pub(crate) struct TreeTable {
    pub(super) tree: Tree,
    pub(super) rows: Vec<TreeRow>,
    /// Rows whose batches load as the viewport approaches them.
    boundaries: Vec<Boundary>,
    /// The rows the table last showed.
    shown: Range<usize>,
    pub(super) filter: String,
    /// Catalog folders holding matches for `filter`, in an indexed tree.
    pub(super) descendants: Option<Arc<Descendants>>,
    /// Supersedes a catalog match query when the filter changes again.
    pub(super) matching: Ticket,
    pub(super) selected: Option<TreeId>,
    pub(super) restore: Option<(Option<TreeId>, Viewport)>,
    pub(super) workers: Arc<async_lock::Semaphore>,
    pub(crate) layout: Layout,
    pub(crate) sort: Option<(Field, ColumnSort)>,
    columns: ColumnInteraction,
    browser: WeakEntity<Browser>,
    pub(crate) router: Option<Entity<Router>>,
    catalog: Entity<Catalog>,
    player: Entity<crate::player::Player>,
}

impl TreeTable {
    pub(crate) fn new(
        browser: WeakEntity<Browser>,
        catalog: Entity<Catalog>,
        player: Entity<crate::player::Player>,
        layout: Layout,
    ) -> Self {
        Self {
            tree: Tree::default(),
            rows: Vec::new(),
            boundaries: Vec::new(),
            shown: 0..0,
            filter: String::new(),
            descendants: None,
            matching: Ticket::default(),
            selected: None,
            restore: None,
            workers: Arc::new(async_lock::Semaphore::new(2)),
            sort: None,
            layout,
            columns: ColumnInteraction::default(),
            browser,
            catalog,
            player,
            router: None,
        }
    }

    pub(super) fn position(&self, id: &TreeId) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.node().is_some_and(|node| node.id == *id))
    }

    pub(crate) fn row_key(&self, index: usize) -> Option<RowKey> {
        self.rows
            .get(index)?
            .node()
            .map(|node| RowKey::Tree(node.id.clone()))
    }

    pub(crate) fn selected_key(&self) -> Option<RowKey> {
        self.selected.clone().map(RowKey::Tree)
    }

    pub(crate) fn on_selection(&mut self, index: usize) {
        let id = self
            .rows
            .get(index)
            .and_then(TreeRow::node)
            .map(|node| node.id.clone());
        if id != self.selected {
            self.restore = None;
            self.selected = id;
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selected = None;
        self.restore = None;
    }

    fn restore_view(table: &mut TableState<Self>, cx: &mut Context<TableState<Self>>) {
        let d = table.delegate();
        let Some((selected, viewport)) = &d.restore else {
            return;
        };
        let row = selected.as_ref().and_then(|id| d.position(id));
        let ready = d.tree.pending.is_empty() && d.tree.next_request().is_none();
        let visible_anchor = viewport
            .anchor
            .as_ref()
            .is_none_or(|key| d.row_position(key).is_some());
        if !ready && (selected.is_some() && row.is_none() || !visible_anchor) {
            return;
        }
        let viewport = viewport.clone();
        table.delegate_mut().restore = None;
        reselect(table, row, cx);
        viewport.restore(table, crate::table::ROW_HEIGHT, |key| {
            table.delegate().row_position(key)
        });
    }

    pub(crate) fn row_position(&self, key: &RowKey) -> Option<usize> {
        match key {
            RowKey::Tree(id) => self.position(id),
            RowKey::Key(id) => self.position(&TreeId::IndexedTrack(*id)),
            RowKey::Disc(id) => self.position(&TreeId::Disc(id.clone())),
            RowKey::Release(id) => self.position(&TreeId::Release(id.clone())),
            _ => None,
        }
    }

    pub(crate) fn restore_bookmark(
        table: &mut TableState<Self>,
        selected: Option<&RowKey>,
        viewport: Viewport,
        cx: &mut Context<TableState<Self>>,
    ) {
        let id = match selected {
            Some(RowKey::Tree(id)) => Some(id.clone()),
            Some(RowKey::Disc(id)) => Some(TreeId::Disc(id.clone())),
            Some(RowKey::Release(id)) => Some(TreeId::Release(id.clone())),
            Some(RowKey::Key(id)) => Some(TreeId::IndexedTrack(*id)),
            _ => None,
        };
        table.delegate_mut().selected = id.clone();
        table.delegate_mut().restore = Some((id, viewport));
        Self::restore_view(table, cx);
    }

    pub(super) fn project(table: &mut TableState<Self>, cx: &mut Context<TableState<Self>>) {
        let height = crate::table::ROW_HEIGHT;
        let d = table.delegate();
        let view = Viewport::capture(table, height, |index| d.row_key(index));
        let descendants = d
            .descendants
            .as_deref()
            .filter(|matches| matches.query == d.filter);
        let (rows, boundaries) = d
            .tree
            .visible_with(d.sort, &d.filter, descendants, &|track| {
                d.catalog.read(cx).mark_of(track)
            });
        table.delegate_mut().boundaries = boundaries;
        table.delegate_mut().rows = rows;
        let d = table.delegate();
        let position = d.selected.as_ref().and_then(|id| d.position(id));
        reselect(table, position, cx);
        view.restore(table, height, |key| table.delegate().row_position(key));
        Self::restore_view(table, cx);
        table.delegate().request_near(cx);
        cx.notify();
    }

    /// Asks the browser for every batch near the rows the table shows.
    fn request_near(&self, cx: &mut App) {
        let wants: Vec<_> = self
            .boundaries
            .iter()
            .filter(|boundary| boundary.near(&self.shown))
            .map(|boundary| (boundary.branch.clone(), boundary.want))
            .collect();
        if wants.is_empty() {
            return;
        }
        let browser = self.browser.clone();
        cx.defer(move |cx| {
            let _ = browser.update(cx, |b, cx| b.load_tree(wants, cx));
        });
    }

    fn field(&self, index: usize) -> Field {
        self.layout.fields().nth(index).unwrap_or(Field::Title)
    }

    fn playing(&self, node: &Node, cx: &App) -> bool {
        self.player
            .read(cx)
            .playing_entry()
            .is_some_and(|entry| match &node.item {
                Item::Track { track, .. } => entry.track.same_item(track),
                Item::Disc { disc, .. } => disc.id.contains(&entry.track),
                _ => false,
            })
    }
}

impl TableColumns for TreeTable {
    fn layout(&self) -> &Layout {
        &self.layout
    }
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
    fn interaction(&self) -> &ColumnInteraction {
        &self.columns
    }
    fn interaction_mut(&mut self) -> &mut ColumnInteraction {
        &mut self.columns
    }
    fn default_layout(&self) -> Layout {
        Layout::new(ContextKind::Tree)
    }
    fn browser(&self) -> Option<WeakEntity<Browser>> {
        Some(self.browser.clone())
    }
    fn sort(&self) -> Option<(Field, ColumnSort)> {
        self.sort
    }
    fn available_fields(&self) -> &'static [Field] {
        TREE_FIELDS
    }
    fn sortable_field(&self, _: Field) -> bool {
        true
    }
    fn sort_rows(table: &mut TableState<Self>, field: Field, cx: &mut Context<TableState<Self>>) {
        let d = table.delegate_mut();
        d.sort = next_sort(d.sort, field);
        Self::project(table, cx);
    }
    fn clear_sort_if_hidden(table: &mut TableState<Self>, cx: &mut Context<TableState<Self>>) {
        let d = table.delegate_mut();
        if d.sort.is_some_and(|(field, _)| !d.layout.has(field)) {
            d.sort = None;
            Self::project(table, cx);
        }
    }
}

impl TableDelegate for TreeTable {
    fn columns_count(&self, _: &App) -> usize {
        self.layout.fields().len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }
    fn column(&self, index: usize, _: &App) -> Column {
        self.table_column(index)
    }
    fn move_column(
        &mut self,
        from: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.move_table_column(from, to, window, cx);
    }
    fn render_th(
        &mut self,
        ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.table_heading(ix, cx)
    }
    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.table_tail()
    }

    fn cell_label(&self, row: usize, col: usize, cx: &App) -> Option<SharedString> {
        let field = self.field(col);
        let node = match self.rows.get(row)? {
            TreeRow::Node { node, .. } => node,
            other => {
                return other
                    .text()
                    .filter(|_| field == Field::Title)
                    .map(Into::into);
            }
        };
        let title = node.value(field);
        Some(if field == Field::Title && self.playing(node, cx) {
            format!("{title} · Now playing").into()
        } else {
            title.into()
        })
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        h_flex()
            .size_full()
            .justify_center()
            .items_center()
            .px(px(12.))
            .text_color(materials(cx).text_muted)
            .child(if self.filter.is_empty() {
                "No folders or supported recordings"
            } else {
                "No matches in the open branches"
            })
    }

    fn render_td(
        &mut self,
        index: usize,
        col: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let field = self.field(col);
        let m = *materials(cx);
        let cell = h_flex().size_full().min_w_0().gap(px(6.));
        match &self.rows[index] {
            TreeRow::Message { depth, text, .. } => cell
                .pl(px((*depth).min(10) as f32 * 14. + 22.))
                .text_color(m.text_faint)
                .when(field == Field::Title, |cell| {
                    cell.child(div().truncate().child(text.clone()))
                }),
            TreeRow::Released { .. } => cell,
            TreeRow::Node { node, depth, .. } => {
                let label = node.value(field);
                if field != Field::Title {
                    return cell
                        .when(field.right_aligned(), |d| d.justify_end())
                        .text_color(m.text_muted)
                        .child(div().truncate().child(label));
                }
                let open = self.tree.expanded.contains(&node.id);
                let icon = match &node.item {
                    Item::Collection { .. } => IconName::Library,
                    Item::Folder { .. } if open => IconName::FolderOpen,
                    Item::Folder { .. } => IconName::Folder,
                    Item::Release { .. } => IconName::Layers,
                    Item::Disc { .. } => IconName::Disc3,
                    Item::Track { .. } => IconName::Music,
                };
                let playing = self.playing(node, cx);
                let disclosure = if node.expandable() {
                    let browser = self.browser.clone();
                    let id = node.id.clone();
                    let tooltip = if open { "Collapse" } else { "Expand" };
                    h_flex()
                        .id(("tree-disclosure", stable_element_key(&id)))
                        .w(px(18.))
                        .h(px(22.))
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .rounded(px(3.))
                        .cursor_pointer()
                        .hover(|d| d.bg(m.control))
                        .role(Role::Button)
                        .aria_expanded(open)
                        .aria_label(format!("{tooltip} {label}"))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            let _ = browser.update(cx, |b, cx| b.toggle_tree(id.clone(), cx));
                        })
                        .child(
                            Icon::new(if open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(px(13.))
                            .text_color(m.text_muted),
                        )
                        .into_any_element()
                } else {
                    div().w(px(18.)).flex_shrink_0().into_any_element()
                };
                cell.pl(px((*depth).min(10) as f32 * 14.))
                    .when(
                        matches!(node.item, Item::Collection { online: false, .. }),
                        |d| d.opacity(0.5),
                    )
                    .child(disclosure)
                    .child(
                        Icon::new(icon)
                            .size(px(14.))
                            .text_color(m.text_faint)
                            .flex_shrink_0(),
                    )
                    .child(div().truncate().min_w_0().child(label))
                    .when(playing, |d| d.child(crate::table::playing_icon(cx)))
            }
        }
    }

    fn visible_rows_changed(
        &mut self,
        visible: Range<usize>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.shown = visible;
        self.request_near(cx);
        let browser = self.browser.clone();
        cx.defer_in(window, move |_, _, cx| {
            let _ = browser.update(cx, |_, cx| cx.emit(ViewChanged));
        });
    }

    fn context_menu(
        &mut self,
        index: usize,
        mut menu: PopupMenu,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let Some(node) = self.rows.get(index).and_then(TreeRow::node).cloned() else {
            return menu;
        };
        let place = self
            .browser
            .upgrade()
            .and_then(|b| b.read(cx).tree_root.clone());
        let browser = self.browser.clone();
        if let Some(destination) = node.destination(place) {
            let open = destination.clone();
            let browse = browser.clone();
            menu = menu.item(PopupMenuItem::new("Open").on_click(move |_, w, cx| {
                let destination = open.clone();
                let _ = browse.update(cx, |b, cx| destination.apply(b, w, cx));
            }));
            if let Some(router) = &self.router {
                menu = destination.open_items(menu, router.clone());
            }
            let open = self.tree.expanded.contains(&node.id);
            let label = if open { "Collapse" } else { "Expand" };
            let id = node.id;
            return menu
                .separator()
                .item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    let _ = browser.update(cx, |b, cx| b.toggle_tree(id.clone(), cx));
                }));
        }
        let Item::Track { track, .. } = &node.item else {
            return menu;
        };
        let track = track.clone();
        let single = node.clone();
        let b = browser.clone();
        let play = browser.clone();
        let mut menu = menu
            .item(
                PopupMenuItem::new("Play from Here")
                    .icon(IconName::Play)
                    .on_click(move |_, _, cx| {
                        let _ = play.update(cx, |b, cx| b.play_tree_node(node.clone(), false, cx));
                    }),
            )
            .item(
                PopupMenuItem::new("Play Track Only").on_click(move |_, _, cx| {
                    let _ = b.update(cx, |b, cx| b.play_tree_node(single.clone(), true, cx));
                }),
            );
        for (label, destination) in [
            ("Show in Library", crate::browser::Destination::Library),
            ("Show in Locations", crate::browser::Destination::Locations),
        ] {
            let (browser, track) = (browser.clone(), track.clone());
            menu = menu.item(PopupMenuItem::new(label).on_click(move |_, w, cx| {
                let _ = browser.update(cx, |b, cx| {
                    b.locate(
                        track.clone(),
                        crate::library::DiscContext::Inferred,
                        destination,
                        w,
                        cx,
                    )
                });
            }));
        }
        menu
    }
}
