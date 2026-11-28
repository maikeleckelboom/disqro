use super::Field;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {
    Disc,
    Mixed,
    Favorites,
    Playlist,
    Shelf,
    Tree,
    Artists,
}

pub(crate) const ARTIST_FIELDS: &[Field] = &[Field::Title, Field::Count];
pub(crate) const SHELF_FIELDS: &[Field] = &[
    Field::Title,
    Field::Artist,
    Field::Year,
    Field::Count,
    Field::Length,
    Field::Format,
    Field::Path,
];

pub(crate) const TREE_FIELDS: &[Field] = &[
    Field::Title,
    Field::Number,
    Field::Artist,
    Field::Album,
    Field::Length,
    Field::Format,
    Field::Year,
    Field::Disc,
    Field::Count,
    Field::Path,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Pin {
    Left,
    Right,
    #[default]
    #[serde(other)]
    None,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LayoutColumn {
    pub(super) field: Field,
    pub(super) width: f32,
    #[serde(default)]
    pub(super) pin: Pin,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Layout {
    pub(super) columns: Vec<LayoutColumn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hidden: Vec<LayoutColumn>,
    /// An untouched layout gives spare room to its text columns. The first
    /// column the listener resizes fixes every width as shown.
    #[serde(default = "adaptive")]
    pub(super) adaptive: bool,
    /// The column whose boundary is being dragged. It keeps its width while
    /// the other text columns absorb the change.
    #[serde(skip)]
    pub(super) holding: Option<usize>,
}
fn adaptive() -> bool {
    true
}

impl Layout {
    /// CD headers carry their own cover, so disc tables leave Cover out.
    pub(crate) fn new(context: ContextKind) -> Self {
        use Field::*;
        let fields: &[Field] = match context {
            ContextKind::Disc => &[Number, Title, Artist, Length, Favorite],
            ContextKind::Favorites => &[Cover, Title, Artist, Album, Length],
            ContextKind::Shelf => SHELF_FIELDS,
            ContextKind::Tree => &[Title, Artist, Count, Length, Format],
            ContextKind::Artists => ARTIST_FIELDS,
            _ => &[Cover, Title, Artist, Album, Length, Favorite],
        };
        let mut layout = Self::for_fields(fields);
        if context == ContextKind::Tree {
            layout.columns[0].width = 300.;
        }
        if context == ContextKind::Shelf {
            for column in &mut layout.columns {
                column.width = match column.field {
                    Title => 320.,
                    Artist => 190.,
                    Path => 200.,
                    field => field.width(),
                };
            }
        }
        layout
    }

    pub(crate) fn for_fields(fields: &[Field]) -> Self {
        Self {
            columns: fields.iter().copied().map(LayoutColumn::new).collect(),
            adaptive: true,
            hidden: Vec::new(),
            holding: None,
        }
    }

    pub(crate) fn fields(&self) -> impl ExactSizeIterator<Item = Field> + '_ {
        self.columns.iter().map(|c| c.field)
    }

    /// Keeps every column at the width it is shown at, so resizing one
    /// column moves nothing but its own boundary.
    pub(super) fn freeze(&mut self, shown: &[f32]) {
        if !self.adaptive || shown.len() != self.columns.len() {
            return;
        }
        for (column, width) in self.columns.iter_mut().zip(shown) {
            column.width = width.round();
        }
        self.adaptive = false;
    }

    pub(crate) fn has(&self, field: Field) -> bool {
        self.columns.iter().any(|c| c.field == field)
    }

    /// Title has room within its minimum width for hierarchy even when pinned
    /// or when Cover and Number are hidden.
    /// Where a group header places its artwork and title: the left edges of
    /// the Cover and Title columns, when Cover precedes Title.
    pub(super) fn header_lead(&self, widths: &[f32]) -> Option<(f32, f32, f32)> {
        let title = self.columns.iter().position(|c| c.field == Field::Title)?;
        let cover = self.columns[..title]
            .iter()
            .position(|c| c.field == Field::Cover)?;
        let left = |ix: usize| widths[..ix].iter().sum::<f32>();
        Some((left(cover), widths[cover], left(title)))
    }

    pub(super) fn content_inset(&self, col: usize, grouped: bool) -> f32 {
        // A Cover column before Title already sets grouped tracks in from
        // their header, whose artwork sits in that column.
        if grouped
            && self.columns[col].field == Field::Title
            && !self.columns[..col].iter().any(|c| c.field == Field::Cover)
        {
            28.
        } else {
            0.
        }
    }

    #[cfg(test)]
    fn show_cover(&mut self, shown: bool) {
        if self.has(Field::Cover) != shown {
            self.toggle(Field::Cover);
        }
    }

    pub(super) fn toggle(&mut self, field: Field) {
        if field == Field::Title {
            return;
        }
        if let Some(at) = self.columns.iter().position(|c| c.field == field) {
            self.hidden.push(self.columns.remove(at));
        } else {
            let column = self
                .hidden
                .iter()
                .position(|c| c.field == field)
                .map(|i| self.hidden.remove(i))
                .unwrap_or_else(|| LayoutColumn::new(field));
            let at = self
                .columns
                .iter()
                .rposition(|c| c.pin == column.pin && c.field < field)
                .map_or(0, |i| i + 1);
            self.columns.insert(at, column);
            self.normalize();
        }
    }

    pub(super) fn set_pin(&mut self, field: Field, pin: Pin) {
        if let Some(column) = self.columns.iter_mut().find(|c| c.field == field) {
            column.pin = pin;
            self.normalize();
        }
    }

    fn normalize(&mut self) {
        self.columns.sort_by_key(|c| match c.pin {
            Pin::Left => 0,
            Pin::None => 1,
            Pin::Right => 2,
        });
    }

    pub(super) fn move_column(&mut self, from: usize, to: usize) -> bool {
        if self.columns[from].pin != self.columns[to].pin {
            return false;
        }
        let column = self.columns.remove(from);
        self.columns.insert(to, column);
        true
    }

    /// Keep full-width outer columns fixed while there is room. On overflow,
    /// the inner pins temporarily join the scrollable range. Restore them as
    /// the pane grows. Nothing here is written back to the profile.
    pub(super) fn scrolling(&self, widths: &[f32], viewport: f32) -> std::ops::Range<usize> {
        let mut range = 0..self.columns.len();
        let available = viewport.max(0.);
        let total: f32 = widths.iter().sum();
        let mut budget = if viewport <= 0. || total <= available {
            f32::INFINITY
        } else {
            // Keep a usable strip of scrollable content in a cramped pane.
            available - 64_f32.min(available / 2.)
        };
        while range.end > 0 {
            let i = range.end - 1;
            let c = &self.columns[i];
            if c.pin != Pin::Right || widths[i] > budget {
                break;
            }
            budget -= widths[i];
            range.end = i;
        }
        while range.start < range.end {
            let i = range.start;
            if self.columns[i].pin != Pin::Left || widths[i] > budget {
                break;
            }
            budget -= widths[i];
            range.start += 1;
        }
        // With every column pinned, two flush pin groups share a divider.
        // Let the inner left column scroll just far enough to expose its edge.
        if viewport > 0.
            && range.is_empty()
            && (1..widths.len()).contains(&range.start)
            && available - total <= 1.
        {
            range.start -= 1;
        }
        range
    }

    pub(super) fn widths(&self, viewport: f32) -> Vec<f32> {
        let mut widths: Vec<_> = self
            .columns
            .iter()
            .map(|c| c.width.clamp(c.field.min_width(), c.field.max_width()))
            .collect();
        if viewport <= 0. {
            return widths;
        }
        let budget = viewport;
        let total: f32 = widths.iter().sum();
        if self.adaptive && total < budget {
            // Give spare space to unmodified text columns. Album editions and
            // artist names benefit from room on a wide display too.
            // Distribute whole pixels in field order. Moving a column to
            // another pin group cannot change which field gets the remainder.
            let eligible: Vec<_> = [(Field::Title, 2.), (Field::Artist, 1.), (Field::Album, 1.)]
                .into_iter()
                .filter_map(|(field, weight)| {
                    self.columns
                        .iter()
                        .position(|c| c.field == field && c.width == field.width())
                        .map(|i| (i, weight))
                })
                .collect();
            if !eligible.is_empty() {
                distribute(&mut widths, &eligible, budget - total);
                return widths;
            }
        }
        // The text columns are the table's elastic part: they share the
        // pane in the proportions the listener gave them, growing into spare
        // room and giving way before the table scrolls. Fixed columns such
        // as Time, Rating and Favorite keep their width. The column being
        // resized holds still, so only its own boundary follows the pointer.
        let elastic: Vec<(usize, f32)> = self
            .columns
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                matches!(c.field, Field::Title | Field::Artist | Field::Album)
                    && Some(*i) != self.holding
            })
            .map(|(i, _)| (i, widths[i]))
            .collect();
        if total < budget {
            distribute(&mut widths, &elastic, budget - total);
        } else if total > budget {
            let give: Vec<(usize, f32)> = elastic
                .iter()
                .map(|&(i, w)| (i, (w - self.columns[i].field.min_width()).max(0.)))
                .filter(|(_, give)| *give > 0.)
                .collect();
            let capacity: f32 = give.iter().map(|(_, g)| g).sum();
            let cut = (total - budget).min(capacity);
            distribute(&mut widths, &give, -cut);
        }
        widths
    }

    /// Fixes every column at the width it is shown at once a resize ends, so
    /// the boundary stays where it was released.
    pub(super) fn settle(&mut self, shown: &[f32]) {
        self.holding = None;
        if self.adaptive || shown.len() != self.columns.len() {
            return;
        }
        for (column, width) in self.columns.iter_mut().zip(shown) {
            column.width = width.round();
        }
        self.adaptive = false;
    }
}

/// Adds `amount` whole pixels (negative to remove) across `weighted`
/// columns in proportion to their weights, in column order.
fn distribute(widths: &mut [f32], weighted: &[(usize, f32)], amount: f32) {
    let mut remaining_weight: f32 = weighted.iter().map(|(_, w)| w).sum();
    let mut remaining = amount;
    for &(i, weight) in weighted {
        if remaining_weight <= 0. {
            break;
        }
        let extra = (remaining * weight / remaining_weight).round();
        widths[i] += extra;
        remaining -= extra;
        remaining_weight -= weight;
    }
}

impl LayoutColumn {
    pub(super) fn new(field: Field) -> Self {
        Self {
            field,
            width: field.width(),
            pin: Pin::None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Layouts {
    #[serde(default)]
    version: u32,
    disc: Layout,
    mixed: Layout,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    favorites: Option<Layout>,
    playlist: Layout,
    #[serde(default = "shelf_layout")]
    pub(crate) shelf: Layout,
    #[serde(default = "tree_layout")]
    pub(crate) tree: Layout,
    #[serde(default = "artists_layout")]
    pub(crate) artists: Layout,
}

fn shelf_layout() -> Layout {
    Layout::new(ContextKind::Shelf)
}
fn tree_layout() -> Layout {
    Layout::new(ContextKind::Tree)
}
fn artists_layout() -> Layout {
    Layout::new(ContextKind::Artists)
}

impl Default for Layouts {
    fn default() -> Self {
        Self {
            version: 3,
            disc: Layout::new(ContextKind::Disc),
            mixed: Layout::new(ContextKind::Mixed),
            favorites: None,
            playlist: Layout::new(ContextKind::Playlist),
            shelf: shelf_layout(),
            tree: tree_layout(),
            artists: artists_layout(),
        }
    }
}
impl Layouts {
    pub(super) fn get(&self, context: ContextKind) -> &Layout {
        match context {
            ContextKind::Disc => &self.disc,
            ContextKind::Mixed => &self.mixed,
            ContextKind::Favorites => self
                .favorites
                .as_ref()
                .expect("initialized Favorites layout"),
            ContextKind::Playlist => &self.playlist,
            ContextKind::Shelf => &self.shelf,
            ContextKind::Tree => &self.tree,
            ContextKind::Artists => &self.artists,
        }
    }
    pub(super) fn get_mut(&mut self, context: ContextKind) -> &mut Layout {
        match context {
            ContextKind::Disc => &mut self.disc,
            ContextKind::Mixed => &mut self.mixed,
            ContextKind::Favorites => self
                .favorites
                .as_mut()
                .expect("initialized Favorites layout"),
            ContextKind::Playlist => &mut self.playlist,
            ContextKind::Shelf => &mut self.shelf,
            ContextKind::Tree => &mut self.tree,
            ContextKind::Artists => &mut self.artists,
        }
    }

    /// Start Favorites with the current track layout and give its own
    /// column edits a separate home in the browser profile.
    pub(super) fn initialize(&mut self, context: ContextKind) {
        if context == ContextKind::Favorites {
            self.favorites.get_or_insert_with(|| {
                let mut layout = self.mixed.clone();
                layout
                    .columns
                    .retain(|column| column.field != Field::Favorite);
                layout
                    .hidden
                    .retain(|column| column.field != Field::Favorite);
                layout.holding = None;
                layout
            });
        }
    }

    pub fn validate(mut self) -> Self {
        for context in [
            ContextKind::Disc,
            ContextKind::Mixed,
            ContextKind::Favorites,
            ContextKind::Playlist,
            ContextKind::Shelf,
            ContextKind::Tree,
            ContextKind::Artists,
        ] {
            if context == ContextKind::Favorites && self.favorites.is_none() {
                continue;
            }
            let layout = self.get_mut(context);
            let mut seen = Vec::new();
            let mut valid = |c: &mut LayoutColumn| {
                let allowed = match context {
                    ContextKind::Shelf => SHELF_FIELDS,
                    ContextKind::Tree => TREE_FIELDS,
                    ContextKind::Artists => ARTIST_FIELDS,
                    _ => &Field::ALL,
                };
                if !allowed.contains(&c.field)
                    || seen.contains(&c.field)
                    || (context == ContextKind::Favorites && c.field == Field::Favorite)
                {
                    return false;
                }
                seen.push(c.field);
                if !(24. ..=c.field.max_width()).contains(&c.width) {
                    c.width = c.field.width();
                }
                true
            };
            layout.columns.retain_mut(&mut valid);
            layout.hidden.retain_mut(valid);
            layout.hidden.retain(|c| c.field != Field::Title);
            if !layout.has(Field::Title) {
                layout.columns.push(LayoutColumn::new(Field::Title));
            }
            layout.normalize();
        }
        self.version = 3;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fields(layout: &Layout) -> Vec<Field> {
        layout.columns.iter().map(|c| c.field).collect()
    }
    #[test]
    fn favorites_inherits_the_tracks_layout_and_keeps_saved_preferences_separate() {
        use Field::*;
        for visible in [false, true] {
            let mut layouts = Layouts::default();
            layouts.mixed.set_pin(Favorite, Pin::Right);
            let favorite = layouts
                .mixed
                .columns
                .iter_mut()
                .find(|column| column.field == Favorite)
                .unwrap();
            favorite.width = 123.;
            if !visible {
                layouts.mixed.toggle(Favorite);
            }
            layouts.mixed.set_pin(Artist, Pin::Left);
            layouts.mixed.toggle(Album);
            let mixed = serde_json::to_value(&layouts.mixed).unwrap();
            let inherited: Vec<_> = layouts
                .mixed
                .fields()
                .filter(|field| *field != Favorite)
                .collect();

            layouts.initialize(ContextKind::Favorites);
            let favorites = layouts.get_mut(ContextKind::Favorites);
            assert_eq!(fields(favorites), inherited);
            assert_eq!(favorites.columns[0].pin, Pin::Left);
            assert!(!favorites.has(Favorite));
            assert!(
                favorites
                    .hidden
                    .iter()
                    .all(|column| column.field != Favorite)
            );
            favorites.toggle(Artist);

            let reread: Layouts =
                serde_json::from_value(serde_json::to_value(layouts).unwrap()).unwrap();
            let reread = reread.validate();
            assert_eq!(
                serde_json::to_value(reread.get(ContextKind::Mixed)).unwrap(),
                mixed
            );
            assert!(!reread.get(ContextKind::Favorites).has(Artist));
            assert!(!reread.get(ContextKind::Favorites).has(Favorite));
        }
    }

    #[test]
    fn shelf_and_artist_layouts_keep_their_edits_separate_from_tracks() {
        use Field::*;
        let mut layouts = Layouts::default();
        layouts.mixed.toggle(Artist);
        let mut restored =
            serde_json::from_value::<Layouts>(serde_json::to_value(&layouts).unwrap())
                .unwrap()
                .validate();
        assert_eq!(fields(&restored.shelf), SHELF_FIELDS);
        assert!(!restored.mixed.has(Artist));
        restored.shelf.set_pin(Path, Pin::Right);
        restored.shelf.toggle(Format);
        restored.artists.toggle(Count);
        assert!(restored.shelf.move_column(1, 0));
        assert_eq!(fields(&restored.shelf)[0], Artist);
        let edited = serde_json::to_vec(&restored).unwrap();
        let again: Layouts = serde_json::from_slice(&edited).unwrap();
        assert!(!again.shelf.has(Format));
        assert!(again.shelf.has(Count) && !again.artists.has(Count));
        assert_eq!(again.shelf.columns.last().unwrap().pin, Pin::Right);
        assert_eq!(fields(&again.mixed), fields(&layouts.mixed));
    }

    #[test]
    fn reset_restores_each_context_default() {
        assert_eq!(
            fields(&Layout::new(ContextKind::Disc)),
            [
                Field::Number,
                Field::Title,
                Field::Artist,
                Field::Length,
                Field::Favorite
            ]
        );
        for context in [ContextKind::Mixed, ContextKind::Playlist] {
            assert_eq!(
                fields(&Layout::new(context)),
                [
                    Field::Cover,
                    Field::Title,
                    Field::Artist,
                    Field::Album,
                    Field::Length,
                    Field::Favorite
                ]
            );
        }
        let layouts = Layouts::default().validate();
        assert!(!layouts.disc.has(Field::Cover));
        assert!(layouts.mixed.has(Field::Cover));
    }
    #[test]
    fn narrow_pane_releases_inner_pins_without_changing_any_saved_width_or_pin() {
        use Field::*;
        let mut layout = Layout::new(ContextKind::Mixed);
        layout.columns[0].width = Cover.max_width();
        layout.set_pin(Cover, Pin::Left);
        layout.set_pin(Artist, Pin::Left);
        layout.set_pin(Favorite, Pin::Right);
        layout.set_pin(Album, Pin::Right);
        layout.adaptive = false;
        let saved = serde_json::to_value(&layout).unwrap();
        let scrolling = |viewport: f32| layout.scrolling(&layout.widths(viewport), viewport);
        // Text columns give way to their minimums first. Then the right pair
        // still fits, and the inner left pin scrolls before the outer one.
        assert_eq!(scrolling(200.), 0..5);
        assert_eq!(scrolling(202.), 0..4);
        assert_eq!(scrolling(334.), 1..4);
        assert_eq!(scrolling(420.), 2..4);
        assert_eq!(scrolling(1000.), 2..4);
        assert_eq!(layout.widths(200.), layout.widths(420.));
        assert_eq!(serde_json::to_value(layout).unwrap(), saved);
    }
    #[test]
    fn touching_pin_groups_can_expose_both_edges_when_every_column_is_pinned() {
        use Field::*;
        let mut layout = Layout::new(ContextKind::Disc);
        layout
            .columns
            .retain(|c| matches!(c.field, Title | Favorite));
        layout.set_pin(Title, Pin::Left);
        layout.set_pin(Favorite, Pin::Right);
        let widths = layout.widths(700.);
        assert_eq!(layout.scrolling(&widths, 700.), 0..1);
        assert_eq!(layout.scrolling(&widths, 720.), 1..1);
        assert_eq!(layout.columns[0].pin, Pin::Left);
    }
    #[test]
    fn pinning_and_moving_preserve_sibling_order_and_widths() {
        use Field::*;
        let mut l = Layout::new(ContextKind::Mixed);
        l.columns[1].width = 381.;
        l.set_pin(Favorite, Pin::Right);
        l.set_pin(Album, Pin::Right);
        l.set_pin(Title, Pin::Left);
        l.set_pin(Artist, Pin::Left);
        assert_eq!(fields(&l), [Title, Artist, Cover, Length, Album, Favorite]);
        assert!(l.move_column(0, 1));
        assert!(!l.move_column(1, 2));
        assert!(l.move_column(4, 5));
        l.set_pin(Title, Pin::None);
        l.set_pin(Favorite, Pin::None);
        assert!(l.move_column(4, 1));
        assert_eq!(fields(&l), [Artist, Favorite, Title, Cover, Length, Album]);
        assert_eq!(l.columns[2].width, 381.);
        l.set_pin(Favorite, Pin::Left);
        assert!(l.move_column(1, 0));
        assert_eq!(fields(&l)[..2], [Favorite, Artist]);
    }
    #[test]
    fn context_pins_round_trip_and_a_hidden_favorite_stays_hidden() {
        use Field::*;
        let mut layouts = Layouts::default();
        layouts.mixed.set_pin(Artist, Pin::Left);
        layouts.mixed.set_pin(Favorite, Pin::None);
        layouts.disc.set_pin(Artist, Pin::Right);
        layouts.playlist.set_pin(Favorite, Pin::Left);
        layouts.playlist.toggle(Favorite);
        let mut json = serde_json::to_value(layouts).unwrap();
        assert_eq!(json["mixed"]["columns"][0]["pin"], "Left");
        assert_eq!(json["mixed"]["columns"][5]["pin"], "None");
        assert_eq!(json["disc"]["columns"][4]["pin"], "Right");
        json["mixed"]["columns"].as_array_mut().unwrap().reverse();
        json["mixed"]["columns"][1]["pin"] = "unknown".into();
        let mut loaded: Layouts = serde_json::from_value(json).unwrap();
        loaded = loaded.validate();
        assert_eq!(
            fields(&loaded.mixed),
            [Artist, Favorite, Length, Album, Title, Cover]
        );
        assert!(!loaded.playlist.has(Favorite));
        loaded.playlist.toggle(Favorite);
        assert_eq!(loaded.playlist.columns[0].pin, Pin::Left);
        assert_eq!(loaded.disc.columns[4].field, Artist);
        for kind in [ContextKind::Disc, ContextKind::Mixed, ContextKind::Playlist] {
            *loaded.get_mut(kind) = Layout::new(kind);
            let cols = &loaded.get(kind).columns;
            assert!(cols[..cols.len() - 1].iter().all(|c| c.pin == Pin::None));
            let last = cols.last().unwrap();
            assert_eq!(
                (last.field, last.pin, last.width),
                (Favorite, Pin::None, 36.)
            );
        }
    }
    #[test]
    fn divider_ownership_tracks_visible_edges_across_pin_groups() {
        use super::super::resize_owner;
        let widths = [48., 160., 60., 88., 90.];
        // Every shared divider in an ordinary table targets its preceding field.
        for i in 1..widths.len() {
            for (host, leading) in [(i - 1, false), (i, true)] {
                assert_eq!(
                    resize_owner(&widths, &(0..5), 446., 0., host, leading),
                    Some(i - 1)
                );
            }
        }
        assert_eq!(resize_owner(&widths, &(0..5), 446., 0., 4, false), Some(4));
        // With two pins on the right their shared boundary grows the outer pin left.
        let owner = |viewport, offset, host, leading| {
            resize_owner(&widths, &(1..3), viewport, offset, host, leading)
        };
        for (host, leading) in [(3, false), (4, true)] {
            assert_eq!(owner(446., 0., host, leading), Some(4));
        }
        // Flush, clipped, and separated positions of the last scrollable edge.
        for (host, leading) in [(2, false), (3, true)] {
            assert_eq!(owner(400., -46., host, leading), Some(3));
        }
        assert_eq!(owner(400., -40., 2, false), None);
        assert_eq!(owner(400., -58., 2, false), Some(2));
        assert_eq!(owner(400., -58., 3, true), Some(3));
        assert_eq!(owner(400., -58., 1, true), None);
    }

    #[test]
    fn pinning_keeps_each_adaptive_columns_displayed_width() {
        let mut layout = Layout::new(ContextKind::Mixed);
        let original = fields(&layout);
        let widths = layout.widths(1001.);
        layout.set_pin(Field::Artist, Pin::Left);
        layout.set_pin(Field::Title, Pin::Right);
        for (column, shown) in layout.columns.iter().zip(layout.widths(1001.)) {
            let index = original
                .iter()
                .position(|field| *field == column.field)
                .unwrap();
            assert_eq!(shown, widths[index]);
        }
    }

    #[test]
    fn resizing_one_column_moves_only_its_own_boundary() {
        // An untouched layout fills a wide table.
        let mut layout = Layout::new(ContextKind::Mixed);
        let shown = layout.widths(1200.);
        assert_eq!(shown.iter().sum::<f32>(), 1200.);
        // The first resize keeps every column as shown.
        layout.freeze(&shown);
        assert!(!layout.adaptive);
        assert_eq!(layout.widths(1200.), shown);
        // While Artist is dragged wider it holds its width, and the other
        // text columns give way in proportion. Fixed columns never move.
        layout.holding = Some(2);
        layout.columns[2].width += 120.;
        let during = layout.widths(1200.);
        assert_eq!(during[2], shown[2] + 120.);
        assert_eq!(during.iter().sum::<f32>(), 1200.);
        for ix in [0, 4, 5] {
            assert_eq!(during[ix], shown[ix]);
        }
        assert!(during[1] < shown[1] && during[3] < shown[3]);
        // Releasing keeps the boundary where it was let go.
        layout.settle(&during);
        assert_eq!(layout.holding, None);
        assert_eq!(layout.widths(1200.), during);
        // A wider pane is shared by the text columns, never left empty.
        let wide = layout.widths(2400.);
        assert_eq!(wide.iter().sum::<f32>(), 2400.);
        assert_eq!(wide[0], during[0]);
        assert!(wide[1] > during[1] && wide[2] > during[2] && wide[3] > during[3]);
        assert_eq!(serde_json::to_value(&layout).unwrap()["adaptive"], false);
        // Reset, and layouts saved before the flag existed, stay adaptive.
        assert!(Layout::new(ContextKind::Mixed).adaptive);
        let saved: Layout =
            serde_json::from_str(r#"{"columns":[{"field":"Title","width":200}]}"#).unwrap();
        assert!(saved.adaptive);
    }
    #[test]
    fn cover_keeps_its_width_and_leaves_title_room() {
        let mut layout = Layout::new(ContextKind::Disc);
        layout.show_cover(true);
        assert_eq!(
            fields(&layout)[..3],
            [Field::Number, Field::Cover, Field::Title]
        );
        // The compact cover neither shrinks nor takes spare room.
        for width in [0., 310., 510., 1040., 2400.] {
            assert_eq!(layout.widths(width)[1], Field::Cover.width());
        }
        let wide = Layout::new(ContextKind::Mixed).widths(2400.);
        assert_eq!(wide[0], Field::Cover.width());
        assert!(wide[1] > Field::Title.width());

        // Overflow scrolls without rescaling the thumbnail or its row.
        layout.columns[1].width = Field::Cover.max_width();
        for width in [310., 400., 1040.] {
            assert_eq!(layout.widths(width)[1], Field::Cover.max_width());
        }
        layout.show_cover(false);
        assert_eq!(
            fields(&layout),
            [
                Field::Number,
                Field::Title,
                Field::Artist,
                Field::Length,
                Field::Favorite
            ]
        );
    }
    #[test]
    fn grouped_title_inset_stays_inside_its_own_column() {
        let mut layout = Layout::new(ContextKind::Disc);
        let check = |layout: &Layout, viewport: f32| {
            let widths = layout.widths(viewport);
            for (i, col) in layout.columns.iter().enumerate() {
                assert_eq!(layout.content_inset(i, false), 0.);
                let inset = layout.content_inset(i, true);
                if col.field == Field::Title {
                    assert_eq!(inset, 28.);
                    assert!(widths[i] - 2. * col.field.padding() - inset >= 94.);
                } else {
                    assert_eq!(inset, 0.);
                }
            }
            widths
        };
        check(&layout, 310.);
        layout.show_cover(true);
        assert!(layout.move_column(2, 1));
        layout.set_pin(Field::Title, Pin::Left);
        layout.set_pin(Field::Cover, Pin::Right);
        let title = 0;
        let cover = layout.columns.len() - 1;
        layout.columns[title].width = Field::Title.min_width();
        layout.columns[cover].width = Field::Cover.max_width();
        let narrow = check(&layout, 240.);
        assert_eq!(narrow[title], Field::Title.min_width());
        assert_eq!(narrow[cover], Field::Cover.max_width());
        assert!(layout.scrolling(&narrow, 240.).contains(&title));
        let wide = check(&layout, 1040.);
        let scrolling = layout.scrolling(&wide, 1040.);
        assert!(!scrolling.contains(&title) && !scrolling.contains(&cover));
        layout.show_cover(false);
        check(&layout, 240.);
    }
    #[test]
    fn each_context_keeps_its_own_cover_size() {
        let mut layouts = Layouts::default();
        layouts.disc.show_cover(true);
        layouts.disc.columns[1].width = 88.;
        layouts.mixed.columns[0].width = 64.;
        let json = serde_json::to_string(&layouts).unwrap();
        let layouts = serde_json::from_str::<Layouts>(&json).unwrap().validate();
        let cover = |layout: &Layout| {
            layout
                .columns
                .iter()
                .find(|c| c.field == Field::Cover)
                .map(|c| c.width)
        };
        assert_eq!(cover(&layouts.disc), Some(88.));
        assert_eq!(cover(&layouts.mixed), Some(64.));
        assert_eq!(cover(&layouts.playlist), Some(Field::Cover.width()));
    }
}
