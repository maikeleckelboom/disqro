//! Resolve listening intent independently of display order and filtering.
use super::*;
use crate::library::DiscContext;

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
                let playlist = scope.keys_are_entries().then(|| p.keys.clone());
                let request = player.read(cx).play_request.clone();
                let generation = request.next();
                cx.spawn(async move |cx| {
                    let result: anyhow::Result<Command> = async {
                        if let Some(keys) = playlist {
                            let rows = catalog
                                .read_with(cx, |c, cx| {
                                    c.rows(scope, keys.to_vec(), &request, generation, cx)
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
                        let context = if matches!(scope, Scope::Album(_)) {
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
    pub fn play_disc(&self, id: &DiscId, cx: &mut App) {
        let player = self.player.clone();
        if let DiscId::Library(album) = id {
            let catalog = self.catalog.clone();
            let album = *album;
            let request = player.read(cx).play_request.clone();
            let generation = request.next();
            cx.spawn(async move |cx| {
                let tracks = Catalog::album_tracks(catalog.clone(), album, cx).await;
                if !request.is_current(generation) {
                    return;
                }
                match tracks {
                    Ok(tracks) if !tracks.is_empty() => player.update(cx, |p, cx| {
                        p.command(Command::PlayDisc(tracks, 0, DiscContext::Selected), cx)
                    }),
                    other => catalog.update(cx, |_, cx| {
                        cx.emit(crate::catalog::CatalogEvent::Notice(
                            other.err().map_or_else(
                                || "This CD is unavailable.".into(),
                                |e| format!("{e:#}"),
                            ),
                        ))
                    }),
                }
            })
            .detach();
        } else if let Some(disc) = self.programs.iter().find(|d| &d.id == id) {
            self.player.update(cx, |p, cx| {
                p.command(
                    Command::PlayDisc(disc.tracks.clone().into(), 0, DiscContext::Selected),
                    cx,
                )
            });
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
