//! Indexed destination summaries. Build ranks once per filter or sort, then
//! hydrate only the displayed window. A release and its numbered discs form
//! one sorting unit even when the viewport crosses a page boundary.
use super::*;
use crate::{
    ordered_result::{Snapshot, Writer},
    store::{Store, Ticket},
    table::RowKey,
};

const PAGE: usize = 256;

pub(crate) struct ShelfIndex {
    pub snapshot: Arc<Snapshot>,
    pub count: usize,
    pub duration: f64,
}

impl ShelfIndex {
    pub fn build(
        store: &Store,
        artists: bool,
        filter: &str,
        sort: Option<(Field, ColumnSort)>,
        ticket: Ticket,
        generation: u64,
    ) -> anyhow::Result<Self> {
        let writer = Writer::new()?;
        let mut family = 0;
        let mut count = 0;
        let mut duration = 0.;
        let mut append = |rows: Vec<Shelf>| -> anyhow::Result<()> {
            let Some(parent) = rows.first() else {
                return Ok(());
            };
            anyhow::ensure!(ticket.is_current(generation), crate::store::Canceled);
            let order = sort.map_or_else(Default::default, |(field, _)| shelf_order(parent, field));
            for (member, row) in rows.iter().enumerate() {
                writer.append(&row.key(), row, order.clone(), family, member)?;
            }
            family += 1;
            Ok(())
        };
        if artists {
            store.visit_artists(filter, |artist| {
                count += 1;
                append(vec![Shelf::Artist(Arc::new(artist))])
            })?;
        } else {
            let mut discs = Vec::new();
            let mut group = None;
            store.visit_releases(filter, |album| {
                let key = album
                    .release
                    .as_ref()
                    .map_or(album.id, |release| release.first);
                if group != Some(key) {
                    append(Discs::shelve(std::mem::take(&mut discs)))?;
                    group = Some(key);
                }
                count += album.count;
                duration += album.duration;
                discs.push(Arc::new(Disc::from_album(album)));
                Ok(())
            })?;
            append(Discs::shelve(discs))?;
        }
        let descending = sort.is_some_and(|(_, d)| d == ColumnSort::Descending);
        Ok(Self {
            snapshot: writer.finish(descending, ticket, generation)?,
            count,
            duration,
        })
    }
}

pub(crate) struct ShelfPage {
    pub(super) index: Arc<Snapshot>,
    offset: usize,
    known: HashMap<RowKey, usize>,
    wanted: Range<usize>,
    task: Option<Task<()>>,
}

impl ShelfPage {
    pub fn initial(
        index: Arc<Snapshot>,
        keys: &[RowKey],
        recording: bool,
    ) -> anyhow::Result<(Self, Vec<Shelf>)> {
        let mut known = HashMap::new();
        for key in keys {
            if let Some(rank) = index.position(key)? {
                known.insert(key.clone(), rank);
                if recording
                    && matches!(key, RowKey::Disc(_))
                    && let Some(Shelf::Disc(disc)) = index.rows::<Shelf>(rank, 1)?.first()
                    && let Some(member) = &disc.release
                    && let Some(parent) = index.position(&RowKey::Release(member.key.clone()))?
                {
                    known.insert(RowKey::Release(member.key.clone()), parent);
                }
            }
        }
        let at = keys.iter().find_map(|k| known.get(k)).copied().unwrap_or(0);
        let offset = (at / PAGE).saturating_sub(1) * PAGE;
        let rows = index.rows(offset, PAGE * 3)?;
        let end = offset + rows.len();
        Ok((
            Self {
                index,
                offset,
                known,
                wanted: offset..end,
                task: None,
            },
            rows,
        ))
    }
}

impl Discs {
    pub(crate) fn install_page(&mut self, page: ShelfPage, rows: Vec<Shelf>) {
        self.original = Arc::default();
        self.rows = rows;
        self.page = Some(page);
    }
    pub(crate) fn row(&self, index: usize) -> Option<&Shelf> {
        self.rows
            .get(index.checked_sub(self.page.as_ref().map_or(0, |p| p.offset))?)
    }
    pub(crate) fn key_at(&self, index: usize) -> Option<RowKey> {
        self.row(index).map(Shelf::key).or_else(|| {
            self.page
                .as_ref()?
                .known
                .iter()
                .find(|(_, rank)| **rank == index)
                .map(|(key, _)| key.clone())
        })
    }
    pub(crate) fn position(&self, key: &RowKey) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.key() == *key)
            .map(|i| i + self.page.as_ref().map_or(0, |p| p.offset))
            .or_else(|| self.page.as_ref()?.known.get(key).copied())
    }
    pub(crate) fn recording_disc(&self, id: &DiscId) -> Option<usize> {
        let index = self.position(&RowKey::Disc(id.clone()))?;
        if let Some(Shelf::Disc(disc)) = self.row(index)
            && let Some(member) = &disc.release
            && let Some(parent) = self.position(&RowKey::Release(member.key.clone()))
        {
            return Some(parent);
        }
        Some(index)
    }
    pub(super) fn request_shelf_page(
        &mut self,
        visible: Range<usize>,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(page) = &mut self.page else {
            return;
        };
        let start = (visible.start / PAGE).saturating_sub(1) * PAGE;
        let end = ((visible.end.div_ceil(PAGE) + 1) * PAGE).min(page.index.len);
        if start >= end || page.wanted.start <= visible.start && page.wanted.end >= visible.end {
            return;
        }
        page.wanted = start..end;
        let index = page.index.clone();
        page.task = Some(cx.spawn(async move |table, cx| {
            let loaded = cx
                .background_spawn(async move { index.rows::<Shelf>(start, end - start) })
                .await;
            let _ = table.update(cx, |table, cx| {
                let selected = table
                    .selected_row()
                    .and_then(|i| table.delegate().key_at(i).map(|k| (k, i)));
                let d = table.delegate_mut();
                let Some(page) = &mut d.page else {
                    return;
                };
                if page.wanted != (start..end) {
                    return;
                }
                match loaded {
                    Ok(rows) => {
                        page.known.clear();
                        page.known.extend(selected);
                        page.offset = start;
                        d.rows = rows;
                    }
                    Err(err) => {
                        d.empty = format!("Unable to read results: {err}").into();
                        d.page = None;
                        d.rows.clear();
                    }
                }
                cx.notify();
            });
        }));
    }
}
