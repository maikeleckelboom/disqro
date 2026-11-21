//! Resolve listening intent independently of display order and filtering.
use super::*;
use crate::{collection::release_discs, library::DiscContext};

impl MusicTable {
    pub fn play(&self, row: usize, cx: &mut App) {
        let Some(ix) = self.rows.sections.data_index(row) else {
            return;
        };
        match &self.rows.data {
            Contents::Folder { sorted, .. } => {
                let Some(track) = sorted.get(ix) else { return };
                let context = if self.folder_cd {
                    DiscContext::Selected
                } else {
                    DiscContext::Inferred
                };
                if !context.allows(track) {
                    let single = Arc::from([track.clone()]);
                    self.player
                        .update(cx, |p, cx| p.command(Command::Play(single, 0), cx));
                    return;
                }
                if let Some(disc) = self.programs.iter().find(|d| d.id.contains(track))
                    && let Some(i) = disc.tracks.iter().position(|t| t.same_item(track))
                {
                    let tracks = disc.tracks.clone().into();
                    self.player.update(cx, |p, cx| {
                        p.command(Command::PlayDisc(tracks, i, context), cx)
                    });
                }
            }
            Contents::Library(p) => {
                let Some(&chosen) = p.keys.get(ix) else {
                    return;
                };
                let catalog = self.catalog.clone();
                let player = self.player.clone();
                let scope = p.scope;
                let search = p.search.clone();
                let request = player.read(cx).play_request.clone();
                let generation = request.next();
                cx.spawn(async move |cx| {
                    let result: anyhow::Result<Command> = async {
                        if scope.keys_are_entries() {
                            let store = catalog
                                .read_with(cx, |c, _| c.handle())
                                .ok_or_else(|| anyhow::anyhow!("The library is unavailable."))?;
                            let keys = store
                                .query(&request, generation, move |store| {
                                    Ok(store
                                        .listing(&store::Query {
                                            scope,
                                            search,
                                            order: None,
                                        })?
                                        .keys)
                                })
                                .await?;
                            let rows = catalog
                                .read_with(cx, |c, cx| {
                                    c.rows(scope, keys, &request, generation, cx)
                                })
                                .await?;
                            let mut start = None;
                            let tracks = rows
                                .into_iter()
                                .filter(|r| r.available)
                                .enumerate()
                                .map(|(i, r)| {
                                    if r.key == chosen {
                                        start = Some(i);
                                    }
                                    r.track
                                })
                                .collect();
                            let i = start.ok_or_else(|| {
                                anyhow::anyhow!("The selected playlist entry is unavailable.")
                            })?;
                            return Ok(Command::Play(tracks, i));
                        }
                        let mut rows = catalog
                            .read_with(cx, |c, cx| {
                                c.rows(scope, vec![chosen], &request, generation, cx)
                            })
                            .await?;
                        let target = rows
                            .pop()
                            .filter(|r| r.available)
                            .ok_or_else(|| anyhow::anyhow!("The selected track is unavailable."))?
                            .track;
                        let context = if matches!(scope, Scope::Album(_) | Scope::Release(_)) {
                            DiscContext::Selected
                        } else {
                            DiscContext::Inferred
                        };
                        if !context.allows(&target) {
                            return Ok(Command::Play(Arc::from([target]), 0));
                        }
                        let tracks =
                            Catalog::program(catalog.clone(), target.clone(), context, cx).await?;
                        let i = tracks
                            .iter()
                            .position(|t| target.same_item(t))
                            .ok_or_else(|| anyhow::anyhow!("The selected track is unavailable."))?;
                        Ok(Command::PlayDisc(tracks, i, context))
                    }
                    .await;
                    if request.is_current(generation) {
                        match result {
                            Ok(command) => player.update(cx, |p, cx| p.command(command, cx)),
                            Err(e) => catalog.update(cx, |_, cx| {
                                cx.emit(crate::catalog::CatalogEvent::Notice(format!("{e:#}")))
                            }),
                        }
                    }
                })
                .detach();
            }
        }
    }
    /// Plays one CD, or every CD of a release, from the beginning in authored
    /// order. The queue then ends with it.
    pub fn play_page(&self, page: &Page, cx: &mut App) {
        let player = self.player.clone();
        let unavailable = match page {
            Page::Disc(_) => "This CD is unavailable.",
            Page::Release(_) => "This release is unavailable.",
        };
        let scope = match page {
            Page::Disc(DiscId::Library(album)) => Some(Scope::Album(*album)),
            Page::Release(DiscId::Library(album)) => Some(Scope::Release(*album)),
            _ => None,
        };
        if let Some(scope) = scope {
            let catalog = self.catalog.clone();
            let request = player.read(cx).play_request.clone();
            let generation = request.next();
            cx.spawn(async move |cx| {
                let tracks = Catalog::authored_tracks(catalog.clone(), scope, cx).await;
                if !request.is_current(generation) {
                    return;
                }
                match tracks {
                    Ok(tracks) if !tracks.is_empty() => player.update(cx, |p, cx| {
                        p.command(Command::PlayDisc(tracks, 0, DiscContext::Selected), cx)
                    }),
                    other => catalog.update(cx, |_, cx| {
                        cx.emit(crate::catalog::CatalogEvent::Notice(
                            other
                                .err()
                                .map_or_else(|| unavailable.into(), |e| format!("{e:#}")),
                        ))
                    }),
                }
            })
            .detach();
            return;
        }
        let tracks: Vec<Arc<Track>> = match page {
            Page::Disc(id) => self
                .programs
                .iter()
                .filter(|d| &d.id == id)
                .flat_map(|d| d.tracks.iter().cloned())
                .collect(),
            Page::Release(key) => release_discs(&self.programs, key)
                .iter()
                .flat_map(|d| d.tracks.iter().cloned())
                .collect(),
        };
        if !tracks.is_empty() {
            self.player.update(cx, |p, cx| {
                p.command(
                    Command::PlayDisc(tracks.into(), 0, DiscContext::Selected),
                    cx,
                )
            });
        }
    }
    /// Whether the rows in memory include `track`, as the rows of a short
    /// page such as a release do.
    pub fn lists(&self, track: &Track) -> bool {
        match &self.rows.data {
            Contents::Folder { original, .. } => original.iter().any(|t| t.same_item(track)),
            Contents::Library(p) => p.cache.values().any(|r| r.track.same_item(track)),
        }
    }
    pub fn page_contains(&self, disc: &DiscId, track: &Track) -> bool {
        match (&self.rows.data, disc) {
            (Contents::Library(p), DiscId::Library(_)) => p
                .cache
                .values()
                .next()
                .is_some_and(|r| DiscId::of(&r.track).contains(track)),
            _ => disc.contains(track),
        }
    }
}
