//! Take playback order from the activated list or authored page.
use super::*;
use crate::catalog::ListeningList;

pub(crate) fn play_library(
    catalog: &Entity<Catalog>,
    player: &Entity<Player>,
    list: ListeningList,
    chosen: Option<i64>,
    command: impl FnOnce(TrackList, usize) -> Command + 'static,
    cx: &mut App,
) {
    let ticket = player.read(cx).play_request.clone();
    let generation = ticket.next();
    let pending = catalog
        .read(cx)
        .listening_list(list, chosen, ticket.clone(), generation, cx);
    let catalog = catalog.clone();
    let sender = player.read(cx).sender();
    cx.spawn(async move |cx| {
        let result = pending.await;
        if !ticket.is_current(generation) {
            return;
        }
        let notice = match result {
            Ok((tracks, start, skipped)) => {
                match sender.play_if_current(command(tracks, start), generation) {
                    Ok(true) if skipped > 0 => {
                        Some(format!("{skipped} unavailable queue entries omitted."))
                    }
                    Ok(_) => None,
                    Err(e) => Some(e.to_string()),
                }
            }
            Err(e) => Some(format!("{e:#}")),
        };
        if let Some(text) = notice {
            catalog.update(cx, |_, cx| {
                cx.emit(crate::catalog::CatalogEvent::Notice(text))
            });
        }
    })
    .detach();
}

fn from_chapter(tracks: TrackList, start: usize) -> Command {
    Command::PlayDisc {
        tracks,
        from_chapter: Some(start),
    }
}

#[derive(Clone)]
pub struct ScanningFolder {
    pub path: PathBuf,
    pub recursive: bool,
    pub grouped: bool,
    pub search: String,
}

enum FolderAction {
    Row(Arc<Track>),
    Program(Page, Option<Arc<Track>>),
    All(bool),
    Collection,
}

impl FolderAction {
    fn command(&self, displayed: &TrackList, programs: &[Arc<Disc>]) -> Option<Command> {
        match self {
            Self::Row(chosen) => Some(Command::Play(
                displayed.clone(),
                displayed.iter().position(|t| t.same_item(chosen))?,
            )),
            Self::Program(page, chosen) => {
                let discs = crate::collection::folder_discs(programs, Some(page), true);
                let tracks: TrackList = discs
                    .iter()
                    .flat_map(|d| d.tracks.iter().cloned())
                    .collect();
                let from_chapter = match chosen {
                    Some(t) => Some(tracks.iter().position(|r| r.same_item(t))?),
                    None => None,
                };
                (!tracks.is_empty()).then_some(Command::PlayDisc {
                    tracks,
                    from_chapter,
                })
            }
            Self::All(shuffle) => {
                (!displayed.is_empty()).then(|| Command::PlayList(displayed.clone(), *shuffle))
            }
            Self::Collection => {
                let visible: HashSet<_> = displayed.iter().map(Arc::as_ptr).collect();
                let order = crate::release::ordered(
                    programs
                        .iter()
                        .map(|d| d.release.as_ref().map(|m| (&m.key, m.number))),
                );
                let mut tracks = Vec::with_capacity(displayed.len());
                let mut append = |i: usize| {
                    tracks.extend(
                        programs[i]
                            .tracks
                            .iter()
                            .filter(|t| visible.contains(&Arc::as_ptr(t)))
                            .cloned(),
                    )
                };
                for item in order {
                    match item {
                        crate::release::Ordered::Disc(i) => append(i),
                        crate::release::Ordered::Release(discs) => {
                            discs.into_iter().for_each(&mut append)
                        }
                    }
                }
                (!tracks.is_empty()).then(|| Command::PlayList(tracks.into(), false))
            }
        }
    }
}
impl MusicTable {
    pub fn play_collection(&self, cx: &mut App) {
        match &self.rows.data {
            Contents::Folder { original, .. } => {
                self.play_folder(FolderAction::Collection, original, cx);
            }
            Contents::Library(p) => {
                let Scope::Library(root) = p.scope else {
                    return;
                };
                play_library(
                    &self.catalog,
                    &self.player,
                    ListeningList::Collection(root, p.search.clone()),
                    None,
                    |tracks, _| Command::PlayList(tracks, false),
                    cx,
                );
            }
        }
    }

    pub fn play(&self, row: usize, cx: &mut App) {
        self.play_row(row, false, cx);
    }

    pub(super) fn play_track(&self, row: usize, cx: &mut App) {
        self.play_row(row, true, cx);
    }

    fn play_row(&self, row: usize, track_only: bool, cx: &mut App) {
        let Some(ix) = self.rows.sections.data_index(row) else {
            return;
        };
        match &self.rows.data {
            Contents::Folder { sorted, .. } => {
                let Some(track) = sorted.get(ix) else { return };
                if track_only {
                    let single = Command::Play(Arc::from([track.clone()]), 0);
                    self.player.update(cx, |p, cx| p.command(single, cx));
                } else {
                    let action = match &self.folder_page {
                        Some(page) => FolderAction::Program(page.clone(), Some(track.clone())),
                        None => FolderAction::Row(track.clone()),
                    };
                    self.play_folder(action, sorted, cx);
                }
            }
            Contents::Library(p) => {
                let Some(&chosen) = p.keys.get(ix) else {
                    return;
                };
                let list = ListeningList::for_row(p.scope, &p.keys, chosen, track_only);
                let command = if matches!(list, ListeningList::Disc(_)) {
                    from_chapter
                } else {
                    Command::Play
                };
                play_library(&self.catalog, &self.player, list, Some(chosen), command, cx);
            }
        }
    }

    pub fn play_all(&self, shuffle: bool, cx: &mut App) {
        match &self.rows.data {
            Contents::Folder { sorted, .. } if !sorted.is_empty() => {
                self.play_folder(FolderAction::All(shuffle), sorted, cx);
            }
            Contents::Library(p) => play_library(
                &self.catalog,
                &self.player,
                ListeningList::Displayed(p.scope, p.keys.clone()),
                None,
                move |tracks, _| Command::PlayList(tracks, shuffle),
                cx,
            ),
            _ => {}
        }
    }
    /// Plays one CD, or every CD of a release, from the beginning in authored
    /// order. The queue then ends with it.
    pub fn play_page(&self, page: &Page, cx: &mut App) {
        let scope = match page {
            Page::Disc(DiscId::Library(album)) => Some(Scope::Album(*album)),
            Page::Release(DiscId::Library(album)) => Some(Scope::Release(*album)),
            _ => None,
        };
        if let Some(scope) = scope {
            play_library(
                &self.catalog,
                &self.player,
                ListeningList::Disc(scope),
                None,
                |tracks, _| Command::PlayDisc {
                    tracks,
                    from_chapter: None,
                },
                cx,
            );
            return;
        }
        if let Contents::Folder { sorted, .. } = &self.rows.data {
            self.play_folder(FolderAction::Program(page.clone(), None), sorted, cx);
        }
    }

    fn play_folder(&self, action: FolderAction, displayed: &TrackList, cx: &mut App) {
        let Some(ScanningFolder {
            path,
            recursive,
            grouped,
            search,
        }) = self.scanning_folder.clone()
        else {
            if let Some(command) = action.command(displayed, &self.programs) {
                self.player.update(cx, |p, cx| p.command(command, cx));
            }
            return;
        };
        // The browser scanner may still be enriching tags or walking subfolders.
        // Finish this activation's projection even if the listener navigates away.
        let player = self.player.read(cx);
        let ticket = player.play_request.clone();
        let generation = ticket.next();
        let sender = player.sender();
        let marks = self.catalog.read(cx).folder_marks();
        let sort = self.sort;
        let page = self.folder_page.clone();
        let catalog = self.catalog.clone();
        cx.spawn(async move |cx| {
            let work = ticket.clone();
            let command = cx
                .background_spawn(async move {
                    let cache = crate::profile::location()
                        .and_then(|p| crate::cache::Cache::open(&p.with_file_name("Cache")).ok());
                    let raw =
                        crate::discovery::folder_tracks(&path, recursive, cache.as_ref(), &|| {
                            work.is_current(generation)
                        });
                    if !work.is_current(generation) {
                        return None;
                    }
                    let programs = crate::collection::Discs::group(
                        raw.iter().flatten(),
                        &HashMap::new(),
                        &path,
                    );
                    if matches!(action, FolderAction::Program(..)) {
                        return action.command(&Arc::default(), &programs);
                    }
                    let discs = crate::collection::folder_discs(&programs, page.as_ref(), grouped);
                    let terms = store::Term::parse(&search);
                    let keep = |t: &&Arc<Track>| crate::collection::matches(t, &terms, || marks(t));
                    let mut tracks: TrackList = if discs.is_empty() && page.is_none() {
                        raw.iter().flatten().filter(keep).cloned().collect()
                    } else {
                        discs
                            .iter()
                            .flat_map(|d| &d.tracks)
                            .filter(keep)
                            .cloned()
                            .collect()
                    };
                    sort_folder(&mut tracks, sort, marks);
                    action.command(&tracks, &programs)
                })
                .await;
            if ticket.is_current(generation) {
                let result = command
                    .ok_or_else(|| anyhow::anyhow!("The selected recording is unavailable."))
                    .and_then(|c| sender.play_if_current(c, generation));
                if let Err(e) = result {
                    catalog.update(cx, |_, cx| {
                        cx.emit(crate::catalog::CatalogEvent::Notice(e.to_string()))
                    });
                }
            }
        })
        .detach();
    }
    pub(crate) fn recording_key(&self, track: &Track, library_id: Option<i64>) -> Option<RowKey> {
        self.rows.recording_key(track, library_id)
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

#[cfg(test)]
mod tests {
    use super::{Command, FolderAction, Page, Track, TrackList};
    use crate::collection::Discs;
    use std::{collections::HashMap, path::Path, sync::Arc};

    #[test]
    fn folder_activation_distinguishes_the_list_cd_and_release() {
        let root = Path::new("Music");
        let all: TrackList = (1..=2)
            .flat_map(|disc| {
                (1..=2).map(move |n| {
                    let mut t = Track::file(root.join(format!("Release/CD{disc}/track{n}.wav")));
                    t.disc_number = Some(disc);
                    t.number = Some(n);
                    t.album = format!("Release (CD{disc})");
                    Arc::new(t)
                })
            })
            .collect();
        let discs = Discs::group(all.iter(), &HashMap::new(), root);
        let chosen = all[1].clone();
        let filtered: TrackList = [all[3].clone(), chosen.clone(), all[2].clone()].into();
        let Some(Command::Play(tracks, start)) =
            FolderAction::Row(chosen.clone()).command(&filtered, &discs)
        else {
            panic!("expected list")
        };
        assert_eq!((tracks, start), (filtered.clone(), 1));
        let page = Page::Disc(discs[0].id.clone());
        let release = Page::Release(discs[0].release.as_ref().unwrap().key.clone());
        for (page, expected) in [(page, &all[..2]), (release, &all[..])] {
            let action = FolderAction::Program(page, Some(chosen.clone()));
            let Some(Command::PlayDisc {
                tracks,
                from_chapter,
            }) = action.command(&filtered, &discs)
            else {
                panic!("expected authored program")
            };
            assert_eq!((&tracks[..], from_chapter), (expected, Some(1)));
        }
    }
}
