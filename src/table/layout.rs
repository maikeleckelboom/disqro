use super::Field;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {
    Disc,
    Mixed,
    Playlist,
}

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

#[derive(Deserialize)]
pub struct LegacyColumn {
    field: String,
    width: f32,
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
    /// Earlier layouts drew artwork inside Title. Loading turns the saved
    /// choice into the Cover column.
    #[serde(default, skip_serializing)]
    artwork: Option<bool>,
}
fn adaptive() -> bool {
    true
}

impl Layout {
    /// CD headers carry their own cover, so disc tables leave Cover out.
    pub(super) fn new(context: ContextKind) -> Self {
        use Field::*;
        let fields: &[Field] = match context {
            ContextKind::Disc => &[Number, Title, Artist, Length, Favorite],
            _ => &[Cover, Title, Artist, Album, Length, Favorite],
        };
        Self {
            columns: fields.iter().copied().map(LayoutColumn::new).collect(),
            adaptive: true,
            hidden: Vec::new(),
            artwork: None,
        }
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

    pub(super) fn has(&self, field: Field) -> bool {
        self.columns.iter().any(|c| c.field == field)
    }

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
        // Stable partition. A right-pinned Favorite owns the outside edge.
        self.columns.sort_by_key(|c| match (c.pin, c.field) {
            (Pin::Left, _) => 0,
            (Pin::None, _) => 1,
            (Pin::Right, Field::Favorite) => 3,
            (Pin::Right, _) => 2,
        });
    }

    pub(super) fn move_column(&mut self, from: usize, to: usize) -> bool {
        let source = &self.columns[from];
        let target = &self.columns[to];
        if source.pin != target.pin
            || (source.pin == Pin::Right
                && (source.field == Field::Favorite || target.field == Field::Favorite))
        {
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
        let mut budget = if viewport <= 0. || widths.iter().sum::<f32>() <= available {
            f32::INFINITY
        } else {
            // Keep a usable strip of scrollable content in a cramped pane.
            available - 64_f32.min(available / 2.)
        };
        while range.end > 0 {
            let i = range.end - 1;
            let c = &self.columns[i];
            if c.pin != Pin::Right || (widths[i] > budget && c.field != Field::Favorite) {
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
            let mut remaining_weight: f32 = eligible.iter().map(|(_, w)| w).sum();
            let mut remaining = budget - total;
            for (i, weight) in eligible {
                let extra = (remaining * weight / remaining_weight).round();
                widths[i] += extra;
                remaining -= extra;
                remaining_weight -= weight;
            }
        }
        widths
    }
}

impl LayoutColumn {
    pub(super) fn new(field: Field) -> Self {
        Self {
            field,
            width: field.width(),
            pin: if field == Field::Favorite {
                Pin::Right
            } else {
                Pin::None
            },
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Layouts {
    #[serde(default)]
    version: u32,
    disc: Layout,
    mixed: Layout,
    playlist: Layout,
}
impl Default for Layouts {
    fn default() -> Self {
        Self {
            version: 1,
            disc: Layout::new(ContextKind::Disc),
            mixed: Layout::new(ContextKind::Mixed),
            playlist: Layout::new(ContextKind::Playlist),
        }
    }
}
impl Layouts {
    pub(super) fn get(&self, context: ContextKind) -> &Layout {
        match context {
            ContextKind::Disc => &self.disc,
            ContextKind::Mixed => &self.mixed,
            ContextKind::Playlist => &self.playlist,
        }
    }
    pub(super) fn get_mut(&mut self, context: ContextKind) -> &mut Layout {
        match context {
            ContextKind::Disc => &mut self.disc,
            ContextKind::Mixed => &mut self.mixed,
            ContextKind::Playlist => &mut self.playlist,
        }
    }

    pub fn validate(mut self) -> Self {
        let migrate_favorite = self.version == 0;
        for context in [ContextKind::Disc, ContextKind::Mixed, ContextKind::Playlist] {
            let layout = self.get_mut(context);
            let mut seen = Vec::new();
            let mut valid = |c: &mut LayoutColumn| {
                if seen.contains(&c.field) {
                    return false;
                }
                seen.push(c.field);
                if c.field == Field::Favorite || !(24. ..=c.field.max_width()).contains(&c.width) {
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
            if let Some(artwork) = layout.artwork.take() {
                layout.show_cover(artwork);
            }
            if migrate_favorite {
                if !layout.has(Field::Favorite) {
                    layout.toggle(Field::Favorite);
                }
                if let Some(c) = layout
                    .columns
                    .iter_mut()
                    .find(|c| c.field == Field::Favorite)
                {
                    c.pin = Pin::Right;
                }
            }
            layout.normalize();
        }
        self.version = 1;
        self
    }

    pub fn migrate(old: &[LegacyColumn], version: u32) -> Self {
        let mut layouts = Self::default();
        let names: Vec<_> = old.iter().map(|c| c.field.as_str()).collect();
        let standard = names
            == [
                "Number", "Title", "Artist", "Album", "Liked", "Rating", "Length",
            ]
            || (version < 2 && names == ["Number", "Title", "Artist", "Album", "Length"]);
        let mut saved: Vec<LayoutColumn> = Vec::new();
        for c in old {
            let Ok(field) = Field::deserialize(serde_json::Value::String(c.field.clone())) else {
                continue;
            };
            if !saved.iter().any(|s| s.field == field) {
                let width =
                    if field != Field::Favorite && (24. ..=field.max_width()).contains(&c.width) {
                        c.width
                    } else {
                        field.width()
                    };
                saved.push(LayoutColumn {
                    width,
                    ..LayoutColumn::new(field)
                });
            }
        }
        if !saved.iter().any(|c| c.field == Field::Title) {
            return layouts;
        }
        for layout in [&mut layouts.disc, &mut layouts.mixed, &mut layouts.playlist] {
            if standard {
                for col in &mut layout.columns {
                    if let Some(c) = saved.iter().find(|c| c.field == col.field) {
                        col.width = c.width;
                    }
                }
            } else {
                // These single-layout profiles predate Cover. Keep this
                // context's default for it.
                let cover = layout.has(Field::Cover);
                layout.columns.clone_from(&saved);
                layout.show_cover(cover);
                if !layout.has(Field::Favorite) {
                    layout.columns.push(LayoutColumn::new(Field::Favorite));
                }
                layout.normalize();
            }
        }
        layouts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn saved(fields: &[(&str, f32)]) -> Vec<LegacyColumn> {
        fields
            .iter()
            .map(|(name, width)| LegacyColumn {
                field: (*name).into(),
                width: *width,
            })
            .collect()
    }
    fn fields(layout: &Layout) -> Vec<Field> {
        layout.columns.iter().map(|c| c.field).collect()
    }
    #[test]
    fn migration_distributes_old_widths_and_preserves_custom_fields() {
        let legacy = saved(&[
            ("Number", 40.),
            ("Title", 319.),
            ("Artist", 135.),
            ("Album", 150.),
            ("Liked", 34.),
            ("Rating", 100.),
            ("Length", 60.),
        ]);
        let layouts = Layouts::migrate(&legacy, 2);
        assert!(!layouts.disc.has(Field::Cover));
        assert!(layouts.mixed.has(Field::Cover) && layouts.playlist.has(Field::Cover));
        assert_eq!(layouts.disc.columns[1].width, 319.);
        assert_eq!(layouts.mixed.columns[1].width, 319.);
        assert_eq!(layouts.disc.columns.len(), 5);
        let custom = saved(&[
            ("Year", 74.),
            ("Title", 248.),
            ("Path", 470.),
            ("Liked", 34.),
        ]);
        let mut layouts = Layouts::migrate(&custom, 2);
        assert_eq!(
            fields(&layouts.disc),
            [Field::Year, Field::Title, Field::Path, Field::Favorite]
        );
        assert_eq!(
            fields(&layouts.playlist),
            [
                Field::Cover,
                Field::Year,
                Field::Title,
                Field::Path,
                Field::Favorite
            ]
        );
        layouts.disc = Layout::new(ContextKind::Disc);
        assert_eq!(layouts.playlist.columns[3].width, 470.);
    }
    #[test]
    fn saved_artwork_choice_becomes_a_single_cover_column() {
        // Saved while artwork lived inside Title, customized everywhere: CD
        // artwork on, mixed artwork off, and a playlist with a duplicate Cover.
        let json = r#"{
            "disc": {"columns": [{"field": "Number", "width": 44},
                {"field": "Title", "width": 354}, {"field": "Year", "width": 81}],
                "artwork": true},
            "mixed": {"columns": [{"field": "Title", "width": 312},
                {"field": "Path", "width": 410}, {"field": "Artist", "width": 150}],
                "artwork": false},
            "playlist": {"columns": [{"field": "Title", "width": 300},
                {"field": "Cover", "width": 300}, {"field": "Cover", "width": 40},
                {"field": "Album", "width": 160}], "artwork": true}
        }"#;
        let layouts = serde_json::from_str::<Layouts>(json).unwrap().validate();
        assert_eq!(
            fields(&layouts.disc),
            [
                Field::Number,
                Field::Cover,
                Field::Title,
                Field::Year,
                Field::Favorite
            ]
        );
        let widths: Vec<f32> = layouts.disc.columns.iter().map(|c| c.width).collect();
        assert_eq!(widths, [44., 40., 354., 81., 34.]);
        assert_eq!(
            fields(&layouts.mixed),
            [Field::Title, Field::Path, Field::Artist, Field::Favorite]
        );
        assert_eq!(layouts.mixed.columns[1].width, 410.);
        assert_eq!(
            fields(&layouts.playlist),
            [Field::Title, Field::Cover, Field::Album, Field::Favorite]
        );
        assert_eq!(layouts.playlist.columns[1].width, Field::Cover.width());

        // The flag is not written back. Later loads trust the columns, so a
        // Cover the listener hid stays hidden.
        let mut saved = serde_json::to_value(&layouts).unwrap();
        assert!(saved["disc"].get("artwork").is_none());
        saved["disc"]["columns"]
            .as_array_mut()
            .unwrap()
            .retain(|c| c["field"] != "Cover");
        let again = serde_json::from_value::<Layouts>(saved).unwrap().validate();
        assert!(!again.disc.has(Field::Cover));
        assert_eq!(fields(&again.playlist), fields(&layouts.playlist));
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
        layout.set_pin(Album, Pin::Right);
        layout.adaptive = false;
        let saved = serde_json::to_value(&layout).unwrap();
        let widths = layout.widths(304.);
        // The right pair fits. Neither left column fits beside it with room to scroll.
        assert_eq!(layout.scrolling(&widths, 304.), 0..4);
        assert_eq!(layout.scrolling(&widths, 380.), 1..4);
        assert_eq!(layout.scrolling(&widths, 1000.), 2..4);
        assert_eq!(layout.widths(1000.), widths);
        assert_eq!(serde_json::to_value(layout).unwrap(), saved);
    }
    #[test]
    fn pinning_and_moving_preserve_sibling_order_and_widths() {
        use Field::*;
        let mut l = Layout::new(ContextKind::Mixed);
        l.columns[1].width = 381.;
        l.set_pin(Album, Pin::Right);
        l.set_pin(Title, Pin::Left);
        l.set_pin(Artist, Pin::Left);
        assert_eq!(fields(&l), [Title, Artist, Cover, Length, Album, Favorite]);
        assert!(l.move_column(0, 1));
        assert!(!l.move_column(1, 2));
        assert!(!l.move_column(4, 5));
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
        assert_eq!(json["disc"]["columns"][3]["pin"], "Right");
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
        assert_eq!(loaded.disc.columns[3].field, Artist);
        for kind in [ContextKind::Disc, ContextKind::Mixed, ContextKind::Playlist] {
            *loaded.get_mut(kind) = Layout::new(kind);
            let cols = &loaded.get(kind).columns;
            assert!(cols[..cols.len() - 1].iter().all(|c| c.pin == Pin::None));
            let last = cols.last().unwrap();
            assert_eq!(
                (last.field, last.pin, last.width),
                (Favorite, Pin::Right, 34.)
            );
        }
    }
    #[test]
    fn compact_action_and_last_metadata_share_one_boundary() {
        use Field::*;
        let mut layouts = Layouts::default();
        layouts.disc.columns.last_mut().unwrap().width = 62.;
        let layouts = layouts.validate();
        assert_eq!(layouts.disc.columns.last().unwrap().width, 34.);
        for viewport in [567., 658., 659., 660., 991.] {
            let widths = layouts.disc.widths(viewport);
            assert_eq!(widths.iter().sum::<f32>(), viewport);
            assert_eq!(widths[3], Length.width());
            assert_eq!(widths[4], Favorite.width());
        }
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
        assert!(shown.iter().sum::<f32>() > 1188.);
        // The first resize keeps every column as shown...
        layout.freeze(&shown);
        assert!(!layout.adaptive);
        assert_eq!(layout.widths(1200.), shown);
        // ...so widening one column moves nothing else, and the table
        // scrolls rather than squeezing its neighbours.
        layout.columns[3].width += 120.;
        let after = layout.widths(1200.);
        for (ix, (before, now)) in shown.iter().zip(&after).enumerate() {
            let expected = if ix == 3 { before + 120. } else { *before };
            assert_eq!(*now, expected);
        }
        // A resized layout no longer stretches with the window.
        assert_eq!(layout.widths(2400.), after);
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
