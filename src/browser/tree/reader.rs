//! Background reads for both browsing and identity-based reveal.
use super::*;
use crate::store::Handle;

pub(super) struct Reader {
    pub(super) descendants: Option<Arc<Descendants>>,
    pub(super) handle: Option<Handle>,
    pub(super) workers: Arc<async_lock::Semaphore>,
    pub(super) ticket: Ticket,
    pub(super) generation: u64,
}

fn offset(cursor: &Option<Cursor>) -> usize {
    match cursor {
        Some(Cursor::Offset(offset)) => *offset,
        _ => 0,
    }
}

impl Reader {
    pub(super) async fn raw(
        &self,
        source: Source,
        cursor: Option<Cursor>,
        cx: &AsyncApp,
    ) -> anyhow::Result<Branch> {
        let ticket = &self.ticket;
        let generation = self.generation;
        let workers = self.workers.clone();
        anyhow::ensure!(ticket.is_current(generation), crate::store::Canceled);
        match (source, self.handle.clone()) {
            (Source::Snapshot(snapshot), _) => {
                cx.background_spawn(async move { snapshot.page(offset(&cursor)) })
                    .await
            }
            (Source::Loaded(nodes), _) => Ok(Branch::slice(
                &nodes,
                match cursor {
                    Some(Cursor::Offset(offset)) => offset,
                    _ => 0,
                },
            )),
            (Source::Offline, _) => Err(anyhow::anyhow!("This collection is unavailable.")),
            (
                Source::Folder {
                    root: Some(root),
                    path,
                },
                Some(store),
            ) => {
                let contents_at = match &cursor {
                    Some(Cursor::Indexed(IndexedCursor::Contents(at))) => Some(*at),
                    _ => None,
                };
                // Subfolders are read from disk on a worker, never on
                // the catalog thread.
                let dirs = match (contents_at, &cursor) {
                    (Some(_), _) => Ok((Vec::new(), None)),
                    (None, cursor) => {
                        let after = match cursor {
                            Some(Cursor::Indexed(IndexedCursor::Dirs(name))) => Some(name.clone()),
                            _ => None,
                        };
                        let (work, path) = (ticket.clone(), path.clone());
                        cx.background_spawn(async move {
                            let _slot = workers.acquire().await;
                            let current = || work.is_current(generation);
                            anyhow::ensure!(current(), crate::store::Canceled);
                            // An unreadable folder still lists its
                            // indexed recordings.
                            Ok(indexed_dirs(root, &path, after.as_deref(), &current)
                                .unwrap_or_default())
                        })
                        .await
                    }
                };
                match dirs {
                    Ok((dirs, Some(more))) => Ok(indexed_page(dirs, None, Some(more))),
                    Ok((dirs, None)) => {
                        store
                            .query(ticket, generation, move |s| {
                                let (contents, taken, more) = indexed_contents(
                                    s,
                                    root,
                                    &path,
                                    contents_at.unwrap_or_default(),
                                )?;
                                Ok(indexed_page(dirs, Some((contents, taken)), more))
                            })
                            .await
                    }
                    Err(error) => Err(error),
                }
            }
            (Source::Album(id, disc), Some(store)) => {
                let offset = match cursor {
                    Some(Cursor::Offset(offset)) => offset,
                    _ => 0,
                };
                store
                    .query(ticket, generation, move |s| {
                        let (rows, next) = s.disc_page(id, offset, PAGE)?;
                        Ok(Branch::page(
                            rows.into_iter()
                                .filter(|row| row.available)
                                .map(|row| Node::track(row.track, Some(disc.clone())))
                                .collect(),
                            next.map(Cursor::Offset),
                        ))
                    })
                    .await
            }
            (Source::Folder { root: None, path }, _) => {
                let work = ticket.clone();
                let after = match cursor {
                    Some(Cursor::Listing(after)) => Some(after),
                    _ => None,
                };
                cx.background_spawn(async move {
                    let _slot = workers.acquire().await;
                    anyhow::ensure!(work.is_current(generation), crate::store::Canceled);
                    physical_branch(&path, after.as_ref(), &|| work.is_current(generation))
                })
                .await
            }
            _ => Err(anyhow::anyhow!("The library is unavailable.")),
        }
    }
    pub(super) async fn read(
        &self,
        source: Source,
        cursor: Option<Cursor>,
        sort: Option<(Field, ColumnSort)>,
        cx: &AsyncApp,
    ) -> anyhow::Result<Branch> {
        let page = self.ordered(source, cursor, sort, cx).await?;
        if let Some(index) = self.descendants.clone() {
            cx.background_spawn(async move { index.annotate(page) })
                .await
        } else {
            Ok(page)
        }
    }
    async fn ordered(
        &self,
        source: Source,
        cursor: Option<Cursor>,
        sort: Option<(Field, ColumnSort)>,
        cx: &AsyncApp,
    ) -> anyhow::Result<Branch> {
        let page = self.raw(source.clone(), cursor, cx).await?;
        let Some(sort) = sort else {
            return Ok(page);
        };
        if page.more.is_none() {
            return Ok(cx
                .background_spawn(async move {
                    let mut page = page;
                    if let Some(nodes) = &mut page.pages[0].nodes {
                        nodes.sort_by_cached_key(|node| sort_key(node, sort));
                    }
                    page
                })
                .await);
        }
        let mut page = page;
        let mut writer = cx
            .background_spawn(async { snapshot::Writer::new() })
            .await?;
        loop {
            let more = page.more.take();
            writer = cx
                .background_spawn(async move {
                    writer.append_nodes(page, sort.0)?;
                    anyhow::Ok(writer)
                })
                .await?;
            let Some(cursor) = more else {
                break;
            };
            page = self.raw(source.clone(), Some(cursor), cx).await?;
        }
        let ticket = self.ticket.clone();
        let generation = self.generation;
        cx.background_spawn(async move {
            writer
                .finish(sort.1 == ColumnSort::Descending, ticket, generation)?
                .page(0)
        })
        .await
    }
}
