use super::Field;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {
    Disc,
    Mixed,
    Playlist,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LayoutColumn {
    pub(super) field: Field,
    pub(super) width: f32,
}

#[derive(Deserialize)]
pub struct LegacyColumn {
    field: String,
    width: f32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Layout {
    pub(super) columns: Vec<LayoutColumn>,
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
            ContextKind::Disc => &[Number, Title, Artist, Length],
            _ => &[Cover, Title, Artist, Album, Length],
        };
        Self {
            columns: fields.iter().copied().map(LayoutColumn::new).collect(),
            adaptive: true,
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
        if !shown {
            self.columns.retain(|c| c.field != Field::Cover);
        } else if !self.has(Field::Cover) {
            self.columns.insert(0, LayoutColumn::new(Field::Cover));
        }
        self.normalize();
    }

    // The pinned prefix must be contiguous in GPUI Kit. Keep the listener's
    // order among every other column.
    fn normalize(&mut self) {
        self.columns.sort_by_key(|c| match c.field {
            Field::Number => 0,
            Field::Cover => 1,
            Field::Title => 2,
            _ => 3,
        });
    }

    /// The shown widths. Columns keep their own widths and the table scrolls
    /// when they need more room than the viewport. Only the pinned columns
    /// give way, in a narrow window, so the scrolling ones stay reachable.
    pub(super) fn widths(&self, viewport: f32) -> Vec<f32> {
        let mut widths: Vec<_> = self
            .columns
            .iter()
            .map(|c| c.width.clamp(c.field.min_width(), c.field.max_width()))
            .collect();
        if viewport <= 0. {
            return widths;
        }
        let budget = (viewport - 8.).max(0.);
        // The pinned columns before Title. A large cover gives way only in a
        // narrow window, so the title keeps room beside it.
        let mut leading = 0.;
        for (column, width) in self.columns.iter().zip(&mut widths) {
            let min = column.field.min_width();
            match column.field {
                Field::Number => {
                    *width = width.min((budget * 0.14).max(min));
                    leading += *width;
                }
                Field::Cover => {
                    *width = width.min((budget * 0.25).max(min)).floor();
                    leading += *width;
                }
                Field::Title => *width = width.min(((budget - leading) * 0.65).max(min)),
                _ => {}
            }
        }
        let total: f32 = widths.iter().sum();
        if self.adaptive && total < budget {
            // Give spare space to unmodified text columns. Album editions and
            // artist names benefit from room on a wide display too.
            let weights: Vec<f32> = self
                .columns
                .iter()
                .map(|c| {
                    if c.width != c.field.width() {
                        return 0.;
                    }
                    match c.field {
                        Field::Title => 2.,
                        Field::Artist | Field::Album => 1.,
                        _ => 0.,
                    }
                })
                .collect();
            let weight: f32 = weights.iter().sum();
            if weight > 0. {
                for (w, share) in widths.iter_mut().zip(weights) {
                    *w += ((budget - total) * share / weight).floor();
                }
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
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Layouts {
    disc: Layout,
    mixed: Layout,
    playlist: Layout,
}
impl Default for Layouts {
    fn default() -> Self {
        Self {
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
        for context in [ContextKind::Disc, ContextKind::Mixed, ContextKind::Playlist] {
            let layout = self.get_mut(context);
            let mut seen = Vec::new();
            layout.columns.retain_mut(|c| {
                if seen.contains(&c.field) {
                    return false;
                }
                seen.push(c.field);
                if !(24. ..=c.field.max_width()).contains(&c.width) {
                    c.width = c.field.width();
                }
                true
            });
            if !seen.contains(&Field::Title) {
                layout.columns.push(LayoutColumn::new(Field::Title));
            }
            if let Some(artwork) = layout.artwork.take() {
                layout.show_cover(artwork);
            }
            layout.normalize();
        }
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
                let width = if (24. ..=1200.).contains(&c.width) {
                    c.width
                } else {
                    field.width()
                };
                saved.push(LayoutColumn { field, width });
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
        assert_eq!(layouts.disc.columns.len(), 4);
        let custom = saved(&[
            ("Year", 74.),
            ("Title", 248.),
            ("Path", 470.),
            ("Liked", 34.),
        ]);
        let mut layouts = Layouts::migrate(&custom, 2);
        assert_eq!(
            fields(&layouts.disc),
            [Field::Title, Field::Year, Field::Path]
        );
        assert_eq!(
            fields(&layouts.playlist),
            [Field::Cover, Field::Title, Field::Year, Field::Path]
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
            [Field::Number, Field::Cover, Field::Title, Field::Year]
        );
        let widths: Vec<f32> = layouts.disc.columns.iter().map(|c| c.width).collect();
        assert_eq!(widths, [44., 40., 354., 81.]);
        assert_eq!(
            fields(&layouts.mixed),
            [Field::Title, Field::Path, Field::Artist]
        );
        assert_eq!(layouts.mixed.columns[1].width, 410.);
        assert_eq!(
            fields(&layouts.playlist),
            [Field::Cover, Field::Title, Field::Album]
        );
        assert_eq!(layouts.playlist.columns[0].width, Field::Cover.width());

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
            [Field::Number, Field::Title, Field::Artist, Field::Length]
        );
        for context in [ContextKind::Mixed, ContextKind::Playlist] {
            assert_eq!(
                fields(&Layout::new(context)),
                [
                    Field::Cover,
                    Field::Title,
                    Field::Artist,
                    Field::Album,
                    Field::Length
                ]
            );
        }
        let layouts = Layouts::default().validate();
        assert!(!layouts.disc.has(Field::Cover));
        assert!(layouts.mixed.has(Field::Cover));
    }
    #[test]
    fn pinned_columns_leave_the_scrolling_ones_reachable() {
        let mut layout = Layout::new(ContextKind::Disc);
        layout.columns[1].width = 1100.;
        layout.columns.push(LayoutColumn::new(Field::Path));
        for width in [310., 510., 1040.] {
            let sizes = layout.widths(width);
            assert!(sizes[1] >= Field::Title.min_width());
            assert!(sizes[0] + sizes[1] < width - 55.);
            assert!(
                sizes
                    .iter()
                    .zip(&layout.columns)
                    .all(|(s, c)| *s >= c.field.min_width())
            );
        }
        assert_eq!(layout.columns[1].width, 1100.);
        // Columns never squeeze to fit: a narrow table scrolls instead.
        let mixed = Layout::new(ContextKind::Mixed);
        let saved: Vec<f32> = mixed.columns.iter().map(|c| c.width).collect();
        assert_eq!(mixed.widths(490.), saved);
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

        // A large cover keeps its size while other columns scroll, and gives
        // way in a narrow window so the title keeps room.
        let large = Field::Cover.max_width();
        layout.columns[1].width = large;
        layout.columns[2].width = 1100.;
        layout.columns.push(LayoutColumn::new(Field::Path));
        layout.columns.push(LayoutColumn::new(Field::Album));
        assert_eq!(layout.widths(1040.)[1], large);
        for width in [310., 400.] {
            let sizes = layout.widths(width);
            assert!((Field::Cover.min_width()..large).contains(&sizes[1]));
            assert!(sizes[2] >= Field::Title.min_width());
            assert!(sizes[..3].iter().sum::<f32>() <= width);
        }
        assert_eq!(layout.columns[1].width, large);
        layout.columns.truncate(5);
        layout.show_cover(false);
        assert_eq!(
            fields(&layout),
            [Field::Number, Field::Title, Field::Artist, Field::Length]
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
