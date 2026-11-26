use super::*;
use crate::{disclosure::Disclosure, panels::nav::INDENT};
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

/// Space between the disclosure and the icon of a row, as in Locations.
const TREE_GAP: f32 = 4.;
/// Space between the icon and the label.
const LABEL_GAP: f32 = 6.;
/// A message stands where the icon of an item at its depth would.
const MESSAGE_INSET: f32 = crate::disclosure::SIZE + TREE_GAP;

pub(crate) struct TreeTable {
    pub(super) tree: Tree,
    /// The rows shown, with each run of released items as one entry.
    pub(super) rows: Vec<TreeRow>,
    /// The first row of each entry of `rows`.
    starts: Vec<usize>,
    /// The rows `rows` span.
    len: usize,
    /// The row of each item shown.
    index: HashMap<TreeId, usize>,
    /// Rows whose batches load as the viewport approaches them.
    boundaries: Vec<Boundary>,
    /// The rows each page's items occupy.
    spans: Vec<Span>,
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
            starts: Vec::new(),
            len: 0,
            index: HashMap::new(),
            boundaries: Vec::new(),
            spans: Vec::new(),
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
        self.index.get(id).copied()
    }

    pub(super) fn reset(&mut self, expansion: Expansion) {
        self.tree = Tree::default();
        self.tree.set_expansion(expansion);
        self.rows.clear();
        self.starts.clear();
        self.index.clear();
        self.boundaries.clear();
        self.spans.clear();
        self.len = 0;
        self.shown = 0..0;
        self.matching.next();
        self.descendants = None;
        self.selected = None;
        self.restore = None;
    }

    /// The entry shown at row `index`.
    pub(super) fn row(&self, index: usize) -> Option<&TreeRow> {
        let entry = self.starts.partition_point(|start| *start <= index);
        let row = self.rows.get(entry.checked_sub(1)?)?;
        (index < self.starts[entry - 1] + row.span()).then_some(row)
    }

    pub(crate) fn row_key(&self, index: usize) -> Option<RowKey> {
        self.row(index)?
            .node()
            .map(|node| RowKey::Tree(node.id.clone()))
    }

    /// Takes a batch a request read, under the current filter.
    pub(super) fn take(
        &mut self,
        id: Option<TreeId>,
        fetch: &Fetch,
        result: anyhow::Result<Branch>,
        cx: &App,
    ) {
        let catalog = self.catalog.read(cx);
        let marks = |track: &Track| catalog.mark_of(track);
        let lens = Lens::new(&self.filter, self.descendants.as_deref(), &marks);
        let tree = &mut self.tree;
        tree.pending.remove(&id);
        match (fetch, result) {
            (Fetch::First, result) => tree.insert_branch(
                id,
                result.unwrap_or_else(|err| Branch {
                    notice: Some(format!("{err:#}")),
                    ..Default::default()
                }),
                lens.as_ref(),
            ),
            (fetch, Ok(page)) => {
                tree.receive(&id, fetch, page, lens.as_ref());
            }
            (_, Err(err)) => {
                if let Some(branch) = tree.branches.get_mut(&id) {
                    branch.wanted = None;
                    branch.notice = Some(format!("{err:#}"));
                }
            }
        }
    }

    /// Marks the pages near the shown rows as in use.
    fn touch(&mut self) {
        let near =
            self.shown.start.saturating_sub(LOOKAHEAD)..self.shown.end.saturating_add(LOOKAHEAD);
        self.tree.touch(&self.spans, near);
    }

    pub(crate) fn selected_key(&self) -> Option<RowKey> {
        self.selected.clone().map(RowKey::Tree)
    }

    pub(crate) fn on_selection(&mut self, index: usize) {
        let id = self
            .row(index)
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

    pub(super) fn land(
        table: &mut TableState<Self>,
        target: TreeId,
        viewport: Viewport,
        center: bool,
        cx: &mut Context<TableState<Self>>,
    ) {
        let row = table.delegate().position(&target);
        table.delegate_mut().selected = Some(target);
        table.delegate_mut().restore = None;
        reselect(table, row, cx);
        if let Some(row) = row.filter(|_| center) {
            table
                .vertical_scroll_handle
                .scroll_to_item(row, ScrollStrategy::Center);
        } else {
            viewport.restore(table, crate::table::ROW_HEIGHT, |key| {
                table.delegate().row_position(key)
            });
        }
        cx.notify();
    }

    pub(super) fn project(table: &mut TableState<Self>, cx: &mut Context<TableState<Self>>) {
        let height = crate::table::ROW_HEIGHT;
        let d = table.delegate();
        // GPUI consumes a scroll-to-item request during layout. An arriving
        // page may project the table before that layout has happened.
        let deferred = table
            .vertical_scroll_handle
            .0
            .borrow()
            .deferred_scroll_to_item
            .and_then(|scroll| Some((d.row_key(scroll.item_index)?, scroll)));
        let view = Viewport::capture(table, height, |index| d.row_key(index));
        let catalog = d.catalog.clone();
        let d = table.delegate_mut();
        d.tree.set_sort(d.sort);
        d.tree.set_query(&d.filter);
        let marks = |track: &Track| catalog.read(cx).mark_of(track);
        let lens = Lens::new(&d.filter, d.descendants.as_deref(), &marks);
        let Projection {
            rows,
            len,
            boundaries,
            spans,
            ..
        } = d.tree.project(lens.as_ref());
        d.boundaries = boundaries;
        d.spans = spans;
        d.rows = rows;
        d.starts.clear();
        d.index.clear();
        let mut at = 0;
        for row in &d.rows {
            d.starts.push(at);
            if let TreeRow::Node { node, .. } = row {
                d.index.entry(node.id.clone()).or_insert(at);
            }
            at += row.span();
        }
        debug_assert_eq!(at, len);
        d.len = len;
        d.touch();
        let d = table.delegate();
        let position = d.selected.as_ref().and_then(|id| d.position(id));
        reselect(table, position, cx);
        view.restore(table, height, |key| table.delegate().row_position(key));
        if let Some((key, mut scroll)) = deferred
            && let Some(index) = table.delegate().row_position(&key)
        {
            scroll.item_index = index;
            table
                .vertical_scroll_handle
                .0
                .borrow_mut()
                .deferred_scroll_to_item = Some(scroll);
        }
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
            .map(|boundary| (boundary.branch.clone(), boundary.want, boundary.search))
            .collect();
        if wants.is_empty()
            && self.tree.first.is_empty()
            && (self.tree.root.is_none()
                || self.tree.branches.contains_key(&None)
                || self.tree.pending.contains_key(&None))
        {
            return;
        }
        let browser = self.browser.clone();
        cx.defer(move |cx| {
            let _ = browser.update(cx, |b, cx| b.load_tree(wants, cx));
        });
    }

    /// Expand, Collapse, Expand All and Collapse All for item `id` of
    /// branch `parent`, or Expand All and Collapse All for the whole tree.
    /// Actions that would change nothing are disabled.
    fn disclosure_items(
        &self,
        mut menu: PopupMenu,
        id: Option<TreeId>,
        parent: Option<TreeId>,
        open: bool,
        _: &App,
    ) -> PopupMenu {
        let browser = self.browser.clone();
        let item = |label: &'static str, action: ExpandAction, enabled: bool| {
            let (browser, id, parent) = (browser.clone(), id.clone(), parent.clone());
            PopupMenuItem::new(label)
                .disabled(!enabled)
                .on_click(move |_, _, cx| {
                    let _ = browser.update(cx, |b, cx| {
                        b.disclose_tree(action, id.clone(), parent.clone(), cx)
                    });
                })
        };
        let tree = &self.tree;
        if id.is_some() {
            menu = menu.item(item(
                if open { "Collapse" } else { "Expand" },
                ExpandAction::Toggle,
                true,
            ));
        }
        let throughout = tree.expanded_throughout(&id) && (id.is_none() || open);
        let anything = match &id {
            None => tree.has_open_inside(&None) || !self.rows.iter().all(|row| !row.open()),
            Some(_) => open || tree.has_open_inside(&id),
        };
        menu.item(item("Expand All", ExpandAction::ExpandAll, !throughout))
            .item(item("Collapse All", ExpandAction::CollapseAll, anything))
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
        self.len
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
        let node = match self.row(row)? {
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
        let Some(row) = self.row(index) else {
            return cell;
        };
        match row {
            TreeRow::Message { depth, text, .. } => cell
                .pl(px((*depth).min(10) as f32 * INDENT + MESSAGE_INSET))
                .text_color(m.text_faint)
                .when(field == Field::Title, |cell| {
                    cell.child(div().truncate().child(text.clone()))
                }),
            TreeRow::Released { .. } => cell,
            TreeRow::Node {
                node,
                depth,
                open,
                parent,
            } => {
                let open = *open;
                let label = node.value(field);
                if field != Field::Title {
                    return cell
                        .when(field.right_aligned(), |d| d.justify_end())
                        .text_color(m.text_muted)
                        .child(div().truncate().child(label));
                }
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
                    let parent = parent.clone();
                    Disclosure::new(
                        ("tree-disclosure", stable_element_key(&id)),
                        open,
                        label.clone(),
                    )
                    .on_toggle(move |_, cx| {
                        let _ = browser
                            .update(cx, |b, cx| b.toggle_tree(id.clone(), parent.clone(), cx));
                    })
                    .into_any_element()
                } else {
                    Disclosure::slot().into_any_element()
                };
                cell.pl(px((*depth).min(10) as f32 * INDENT))
                    .gap(px(TREE_GAP))
                    .when(
                        matches!(node.item, Item::Collection { online: false, .. }),
                        |d| d.opacity(0.5),
                    )
                    .child(disclosure)
                    .child(
                        Icon::new(icon)
                            .size(px(14.))
                            .mr(px(LABEL_GAP - TREE_GAP))
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
        self.touch();
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
        let Some(TreeRow::Node {
            node, open, parent, ..
        }) = self.row(index).cloned()
        else {
            // Empty space below the rows: the whole tree.
            return self.disclosure_items(menu, None, None, false, cx);
        };
        let place = self
            .browser
            .upgrade()
            .and_then(|b| b.read(cx).tree_root.clone());
        let browser = self.browser.clone();
        if let Some(destination) = node.destination(place) {
            let target = destination.clone();
            let browse = browser.clone();
            menu = menu.item(PopupMenuItem::new("Open").on_click(move |_, w, cx| {
                let destination = target.clone();
                let _ = browse.update(cx, |b, cx| destination.apply(b, w, cx));
            }));
            if let Some(router) = &self.router {
                menu = destination.open_in_new_tab(menu, router.clone());
            }
            return self.disclosure_items(menu.separator(), Some(node.id), parent, open, cx);
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
