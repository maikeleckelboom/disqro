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
    pub(super) artwork: bool,
}

impl Layout {
    pub(super) fn new(context: ContextKind) -> Self {
        use Field::*;
        let fields = match context {
            ContextKind::Disc => [Number, Title, Artist, Length],
            _ => [Title, Artist, Album, Length],
        };
        Self {
            columns: fields.into_iter().map(LayoutColumn::new).collect(),
            artwork: context != ContextKind::Disc,
        }
    }

    // The pinned prefix must be contiguous in GPUI Kit. Keep the listener's
    // order among every other column.
    fn normalize(&mut self) {
        self.columns.sort_by_key(|c| match c.field {
            Field::Number => 0,
            Field::Title => 1,
            _ => 2,
        });
    }

    pub(super) fn widths(&self, viewport: f32) -> Vec<f32> {
        let mut widths: Vec<_> = self
            .columns
            .iter()
            .map(|c| c.width.max(c.field.min_width()))
            .collect();
        if viewport <= 0. {
            return widths;
        }
        let budget = (viewport - 8.).max(0.);
        let mut pinned_number = 0.;
        for (column, width) in self.columns.iter().zip(&mut widths) {
            match column.field {
                Field::Number => {
                    *width = width.min((budget * 0.14).max(column.field.min_width()));
                    pinned_number = *width;
                }
                Field::Title => {
                    *width =
                        width.min(((budget - pinned_number) * 0.65).max(column.field.min_width()))
                }
                _ => {}
            }
        }
        let total: f32 = widths.iter().sum();
        if total > budget {
            let slack: f32 = self
                .columns
                .iter()
                .zip(&widths)
                .map(|(c, w)| (w - c.field.min_width()).max(0.))
                .sum();
            if slack > 0. {
                let fraction = ((total - budget) / slack).min(1.);
                for (c, w) in self.columns.iter().zip(&mut widths) {
                    *w = (*w - (*w - c.field.min_width()).max(0.) * fraction)
                        .floor()
                        .max(c.field.min_width());
                }
            }
        } else if total < budget {
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
                if !(24. ..=1200.).contains(&c.width) {
                    c.width = c.field.width();
                }
                true
            });
            if !seen.contains(&Field::Title) {
                layout.columns.push(LayoutColumn::new(Field::Title));
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
                layout.columns.clone_from(&saved);
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
        assert!(!layouts.disc.artwork);
        assert!(layouts.mixed.artwork && layouts.playlist.artwork);
        assert_eq!(layouts.disc.columns[1].width, 319.);
        assert_eq!(layouts.mixed.columns[0].width, 319.);
        assert_eq!(layouts.disc.columns.len(), 4);
        let custom = saved(&[
            ("Year", 74.),
            ("Title", 248.),
            ("Path", 470.),
            ("Liked", 34.),
        ]);
        let mut layouts = Layouts::migrate(&custom, 2);
        assert_eq!(
            layouts
                .disc
                .columns
                .iter()
                .map(|c| c.field)
                .collect::<Vec<_>>(),
            [Field::Title, Field::Year, Field::Path]
        );
        layouts.disc = Layout::new(ContextKind::Disc);
        assert_eq!(layouts.playlist.columns[2].width, 470.);
    }
    #[test]
    fn fitting_does_not_change_saved_widths_or_consume_scrollable_viewport() {
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
        let mixed = Layout::new(ContextKind::Mixed);
        assert!(mixed.widths(490.).iter().sum::<f32>() <= 490.);
        assert_eq!(layout.columns[1].width, 1100.);
    }
}
