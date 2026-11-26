//! The listening queue: what is playing, what played before it in this
//! session, and what plays next.
//!
//! The queue holds its own copies of the tracks. Browsing, sorting a table or
//! editing a folder never changes it, and changing the queue never changes an
//! album or a playlist. A CUE chapter is an entry like any other file.
use crate::library::{DiscContext, Track};
use crate::store::{EntryId, PlaylistId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistOccurrence {
    pub playlist: PlaylistId,
    pub entry: EntryId,
}

/// An ordered activation or selection. Playlist keys travel alongside the
/// tracks so library recordings keep their own identity.
pub struct Selection {
    pub tracks: crate::library::TrackList,
    pub source: Option<String>,
    playlist: Option<(PlaylistId, Vec<EntryId>)>,
}

impl Selection {
    pub fn from_rows(
        scope: crate::store::Scope,
        rows: impl IntoIterator<Item = crate::store::Row>,
    ) -> Self {
        let mut tracks = Vec::new();
        let mut playlist = match scope {
            crate::store::Scope::Playlist(id) => Some((id, Vec::new())),
            _ => None,
        };
        for row in rows {
            tracks.push(row.track);
            if let Some((_, keys)) = &mut playlist {
                keys.push(row.key);
            }
        }
        Self::from_indexed(tracks.into(), playlist)
    }

    pub fn from_indexed(
        tracks: crate::library::TrackList,
        playlist: Option<(PlaylistId, Vec<EntryId>)>,
    ) -> Self {
        assert!(
            playlist
                .as_ref()
                .is_none_or(|(_, keys)| keys.len() == tracks.len())
        );
        Self {
            tracks,
            source: None,
            playlist,
        }
    }

    pub fn named(mut self, source: Option<String>) -> Self {
        self.source = source;
        self
    }

    fn items(&self) -> impl Iterator<Item = (Arc<Track>, Option<PlaylistOccurrence>)> + '_ {
        self.tracks.iter().enumerate().map(|(i, track)| {
            (
                track.clone(),
                self.playlist
                    .as_ref()
                    .map(|(playlist, entries)| PlaylistOccurrence {
                        playlist: *playlist,
                        entry: entries[i],
                    }),
            )
        })
    }
}

impl From<crate::library::TrackList> for Selection {
    fn from(tracks: crate::library::TrackList) -> Self {
        Self {
            tracks,
            source: None,
            playlist: None,
        }
    }
}

#[cfg(test)]
impl<T: Into<Arc<Track>>> FromIterator<T> for Selection {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        crate::library::TrackList::from_iter(iter.into_iter().map(Into::into)).into()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub track: Arc<Track>,
    #[serde(default)]
    pub disc_context: DiscContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist_occurrence: Option<PlaylistOccurrence>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}
impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Queue {
    /// Entries in play order. Those before `current` are this session's history.
    pub entries: Arc<Vec<Entry>>,
    pub current: Option<usize>,
    /// History can remain when the last current entry is removed.
    #[serde(skip_serializing_if = "is_zero")]
    idle_history: usize,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// The list that started this sequence. Manual additions can mix sources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The order of upcoming entries before shuffle was switched on.
    unshuffled: Arc<Vec<u64>>,
    next_id: u64,
    #[serde(skip)]
    pub revision: u64,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl Queue {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.current.is_none_or(|i| i < self.entries.len()),
            "Queue cursor is outside its entries"
        );
        anyhow::ensure!(
            self.idle_history <= self.entries.len()
                && (self.current.is_none() || self.idle_history == 0),
            "Queue history is outside its entries"
        );
        let mut ids = std::collections::HashSet::new();
        anyhow::ensure!(
            self.next_id < u64::MAX
                && self
                    .entries
                    .iter()
                    .all(|e| e.id > 0 && e.id <= self.next_id && ids.insert(e.id)),
            "Invalid queue entry identities"
        );
        anyhow::ensure!(
            self.entries.iter().all(
                |e| e.track.duration.is_none_or(|s| s.is_finite() && s >= 0.)
                    && e.track
                        .disc
                        .as_ref()
                        .is_none_or(|d| d.end.is_none_or(|v| v > d.start))
            ),
            "Invalid saved track boundaries"
        );
        Ok(())
    }
    fn entry(&mut self, track: impl Into<Arc<Track>>) -> Entry {
        self.next_id += 1;
        Entry {
            id: self.next_id,
            track: track.into(),
            disc_context: DiscContext::Inferred,
            playlist_occurrence: None,
        }
    }
    fn selection_entries(&mut self, selection: &Selection) -> Vec<Entry> {
        selection
            .items()
            .map(|(track, playlist_occurrence)| Entry {
                playlist_occurrence,
                ..self.entry(track)
            })
            .collect()
    }
    fn touch(&mut self) {
        self.revision += 1;
    }
    pub fn current_entry(&self) -> Option<&Entry> {
        self.entries.get(self.current?)
    }
    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }
    /// The first entry that has not played yet.
    fn upcoming_start(&self) -> usize {
        self.current.map_or(self.idle_history, |c| c + 1)
    }
    /// Indices of the occurrences that play after the active one, or after
    /// the history while nothing is active. The active occurrence is Now
    /// Playing, not part of the queue.
    pub fn upcoming_range(&self) -> std::ops::Range<usize> {
        self.upcoming_start().min(self.entries.len())..self.entries.len()
    }
    pub fn upcoming(&self) -> &[Entry] {
        &self.entries[self.upcoming_range()]
    }
    pub fn history(&self) -> &[Entry] {
        &self.entries[..self.current.unwrap_or(self.idle_history)]
    }

    /// Starts a new queue from a table, beginning at `start`. The tracks before
    /// it become history, so Previous moves back through the album.
    pub fn replace(&mut self, selection: Selection, start: usize) {
        self.entries = Arc::new(self.selection_entries(&selection));
        self.source = selection.source;
        self.current = (start < self.entries.len()).then_some(start);
        self.idle_history = 0;
        self.unshuffled = Arc::default();
        if self.shuffle {
            self.shuffle_upcoming();
        }
        self.touch();
    }
    /// Inserts tracks right after the current entry, in the order given.
    pub fn play_next(&mut self, selection: &Selection) -> Vec<u64> {
        let new = self.selection_entries(selection);
        self.insert_next(new)
    }
    fn insert_next(&mut self, new: Vec<Entry>) -> Vec<u64> {
        let at = self.upcoming_start().min(self.entries.len());
        let ids = new.iter().map(|e| e.id).collect::<Vec<_>>();
        Arc::make_mut(&mut self.entries).splice(at..at, new);
        if self.shuffle {
            Arc::make_mut(&mut self.unshuffled).splice(0..0, ids.iter().copied());
        }
        self.touch();
        ids
    }
    pub fn add(&mut self, selection: &Selection) -> Vec<u64> {
        let new = self.selection_entries(selection);
        let ids = new.iter().map(|e| e.id).collect::<Vec<_>>();
        Arc::make_mut(&mut self.entries).extend(new);
        if self.shuffle {
            Arc::make_mut(&mut self.unshuffled).extend(ids.iter().copied());
        }
        self.touch();
        ids
    }
    /// Promote one existing occurrence of each remaining CD track, creating only
    /// missing occurrences. Other entries (including intentional repeats) keep
    /// their relative order. A delayed action belongs to one exact occurrence.
    pub fn continue_disc(
        &mut self,
        entry: u64,
        program: &[Arc<Track>],
        context: DiscContext,
    ) -> bool {
        let Some(start) = self.current else {
            return false;
        };
        let current = &self.entries[start];
        if current.id != entry || !context.allows(&current.track) {
            return false;
        }
        let Some(chapter) = program.iter().position(|t| t.same_item(&current.track)) else {
            return false;
        };
        let remaining = &program[chapter + 1..];
        if remaining.is_empty() {
            return false;
        }
        let ordered = self
            .upcoming()
            .iter()
            .map(|e| &e.track)
            .zip(remaining)
            .all(|(a, b)| a.same_item(b))
            && self.upcoming().len() >= remaining.len();
        let select_disc = context == DiscContext::Selected
            && ordered
            && self.entries[start..=start + remaining.len()]
                .iter()
                .any(|e| !e.disc_context.allows(&e.track));
        if !select_disc && !self.shuffle && self.repeat != Repeat::One && ordered {
            return false;
        }
        // This explicit listening action establishes authored order. Keep the
        // existing order of unrelated upcoming entries, even when shuffled.
        self.shuffle = false;
        self.unshuffled = Arc::default();
        if self.repeat == Repeat::One {
            self.repeat = Repeat::Off;
        }
        if !ordered {
            let start = self.upcoming_start();
            let upcoming = Arc::make_mut(&mut self.entries).split_off(start);
            let mut upcoming: Vec<_> = upcoming.into_iter().map(Some).collect();
            for track in remaining {
                let found = upcoming
                    .iter_mut()
                    .find(|slot| slot.as_ref().is_some_and(|e| e.track.same_item(track)))
                    .and_then(Option::take);
                let next = found.unwrap_or_else(|| self.entry(track.clone()));
                Arc::make_mut(&mut self.entries).push(next);
            }
            Arc::make_mut(&mut self.entries).extend(upcoming.into_iter().flatten());
        }
        if context == DiscContext::Selected {
            for entry in &mut Arc::make_mut(&mut self.entries)[start..=start + remaining.len()] {
                entry.disc_context = context;
            }
        }
        self.touch();
        true
    }
    /// Removes queued and history occurrences. The active occurrence is
    /// never removed: a queue edit cannot stop or replace the recording that
    /// is playing, and a stale request that names it is ignored. Returns
    /// whether anything was removed.
    pub fn remove(&mut self, ids: &[u64]) -> bool {
        let active = self.current_entry().map(|e| e.id);
        let removed = |id: u64| Some(id) != active && ids.contains(&id);
        let history = self.history().iter().filter(|e| removed(e.id)).count();
        let before = self.entries.len();
        Arc::make_mut(&mut self.entries).retain(|e| !removed(e.id));
        if self.entries.len() == before {
            return false;
        }
        match &mut self.current {
            Some(current) => *current -= history,
            None => self.idle_history -= history,
        }
        Arc::make_mut(&mut self.unshuffled).retain(|&id| !removed(id));
        self.touch();
        true
    }
    /// Reorders upcoming entries without crossing the current recording.
    pub fn move_upcoming(&mut self, id: u64, before: Option<u64>) {
        let start = self.upcoming_start();
        let to = match before {
            Some(before) => {
                let Some(to) = self.index_of(before) else {
                    return;
                };
                to
            }
            None => self.entries.len(),
        };
        if to >= start
            && let Some(from) = self.index_of(id).filter(|&from| from >= start)
        {
            self.move_entry(from, to);
        }
    }

    /// Promote an upcoming entry or schedule a fresh occurrence from History.
    pub fn schedule_next(&mut self, id: u64) {
        let Some(from) = self.index_of(id) else {
            return;
        };
        let start = self.upcoming_start();
        if Some(from) == self.current {
            return;
        }
        if from < start {
            let previous = self.entries[from].clone();
            let entry = self.entry(previous.track.clone());
            self.insert_next(vec![Entry {
                id: entry.id,
                ..previous
            }]);
        } else {
            self.move_entry(from, start);
            if self.shuffle {
                let order = Arc::make_mut(&mut self.unshuffled);
                order.retain(|&entry| entry != id);
                order.insert(0, id);
                self.touch();
            }
        }
    }

    fn move_entry(&mut self, from: usize, to: usize) {
        if to == from || to == from + 1 {
            return;
        }
        let entry = Arc::make_mut(&mut self.entries).remove(from);
        let to = if to > from { to - 1 } else { to };
        Arc::make_mut(&mut self.entries).insert(to, entry);
        self.touch();
    }
    pub fn clear_upcoming(&mut self) {
        let start = self.upcoming_start().min(self.entries.len());
        Arc::make_mut(&mut self.entries).truncate(start);
        self.unshuffled = Arc::default();
        self.touch();
    }
    pub fn clear_history(&mut self) {
        let history = self.history().len();
        if history != 0 {
            Arc::make_mut(&mut self.entries).drain(..history);
            self.current = self.current.map(|_| 0);
            self.idle_history = 0;
            self.touch();
        }
    }
    pub fn set_current(&mut self, index: Option<usize>) {
        let index = index.filter(|&i| i < self.entries.len());
        if self.current != index {
            self.idle_history = if index.is_none() {
                self.current.unwrap_or(0)
            } else {
                0
            };
            self.current = index;
            self.touch();
        }
    }
    pub fn set_repeat(&mut self, repeat: Repeat) {
        if self.repeat != repeat {
            self.repeat = repeat;
            self.touch();
        }
    }
    pub fn set_shuffle(&mut self, on: bool) {
        if self.shuffle == on {
            return;
        }
        self.shuffle = on;
        if on {
            self.shuffle_upcoming();
        } else {
            let start = self.upcoming_start().min(self.entries.len());
            let order: std::collections::HashMap<_, _> = std::mem::take(&mut self.unshuffled)
                .iter()
                .copied()
                .enumerate()
                .map(|(i, id)| (id, i))
                .collect();
            Arc::make_mut(&mut self.entries)[start..]
                .sort_by_key(|e| order.get(&e.id).copied().unwrap_or(usize::MAX));
        }
        self.touch();
    }
    fn shuffle_upcoming(&mut self) {
        let start = self.upcoming_start().min(self.entries.len());
        self.unshuffled = Arc::new(self.entries[start..].iter().map(|e| e.id).collect());
        fastrand::shuffle(&mut Arc::make_mut(&mut self.entries)[start..]);
    }

    /// The entry that follows the current one. `ended` is true when the
    /// current track finished by itself, which is when Repeat One applies.
    pub fn next_index(&self, ended: bool) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let Some(c) = self.current else {
            return (self.idle_history < self.entries.len()).then_some(self.idle_history);
        };
        if ended && self.repeat == Repeat::One {
            return Some(c.min(self.entries.len() - 1));
        }
        if c + 1 < self.entries.len() {
            Some(c + 1)
        } else if self.repeat == Repeat::All {
            Some(0)
        } else {
            None
        }
    }
    pub fn previous_index(&self) -> Option<usize> {
        match self.current? {
            0 if self.repeat == Repeat::All && !self.entries.is_empty() => {
                Some(self.entries.len() - 1)
            }
            0 => None,
            c => Some(c - 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::DiscTrack;
    use std::path::PathBuf;

    fn file(name: &str) -> Track {
        Track::file(PathBuf::from(format!("C:\\Music\\{name}.mp3")))
    }
    fn chapter(sheet: &str, n: u8) -> Track {
        let mut t = Track::file(PathBuf::from(format!("C:\\Discs\\{sheet}.flac")));
        t.title = format!("{sheet} {n}");
        t.disc = Some(DiscTrack {
            sheet: PathBuf::from(format!("C:\\Discs\\{sheet}.cue")),
            number: n,
            start: u64::from(n) * 75 * 60,
            end: Some(u64::from(n + 1) * 75 * 60),
        });
        t
    }
    fn titles(q: &Queue) -> Vec<String> {
        q.entries.iter().map(|e| e.track.title.clone()).collect()
    }

    #[test]
    fn mixed_cue_chapters_and_files_keep_their_identity() {
        let mut q = Queue::default();
        q.add(
            &[chapter("A", 7), file("single"), chapter("B", 11)]
                .into_iter()
                .collect(),
        );
        assert_eq!(q.next_index(false), Some(0));
        q.set_current(Some(0));
        assert_eq!(q.next_index(false), Some(1));
        let disc = q.entries[2].track.disc.as_ref().unwrap();
        assert_eq!(
            (disc.number, disc.sheet.file_name().unwrap()),
            (11, "B.cue".as_ref())
        );
        assert!(q.entries[0].track.same_item(&chapter("A", 7)));
    }

    #[test]
    fn play_next_and_add_respect_the_current_entry() {
        let mut q = Queue::default();
        q.replace([file("1"), file("2"), file("3")].into_iter().collect(), 1);
        q.play_next(&[file("x"), file("y")].into_iter().collect());
        q.add(&[file("z")].into_iter().collect());
        assert_eq!(titles(&q), ["1", "2", "x", "y", "3", "z"]);
        assert_eq!(q.current, Some(1));
        assert_eq!(q.history().len(), 1);
        assert_eq!(q.upcoming().len(), 4);
    }

    #[test]
    fn reordering_removing_and_clearing() {
        let mut q = Queue::default();
        q.replace(
            [file("1"), file("2"), file("3"), file("4")]
                .into_iter()
                .collect(),
            1,
        );
        let id4 = q.entries[3].id;
        let id3 = q.entries[2].id;
        let current = q.entries[1].id;
        q.move_upcoming(id4, Some(current));
        q.move_upcoming(current, None);
        assert_eq!(titles(&q), ["1", "2", "3", "4"]);
        q.move_upcoming(id4, Some(id3));
        assert_eq!(titles(&q), ["1", "2", "4", "3"]);
        let id1 = q.entries[0].id;
        q.move_upcoming(id1, None);
        assert_eq!(titles(&q), ["1", "2", "4", "3"]);
        assert_eq!(q.current_entry().unwrap().track.title, "2");
        assert!(q.remove(&[id3]));
        assert_eq!(titles(&q), ["1", "2", "4"]);
        // The active occurrence is Now Playing, not a queue entry.
        let revision = q.revision;
        assert!(!q.remove(&[current]));
        assert_eq!(q.revision, revision);
        assert_eq!(q.current_entry().unwrap().track.title, "2");
        q.clear_upcoming();
        assert_eq!(titles(&q), ["1", "2"]);
        assert_eq!(q.current_entry().unwrap().id, current);
    }

    #[test]
    fn every_queue_edit_keeps_the_active_occurrence() {
        let mut q = Queue::default();
        let same = file("same");
        q.replace(
            [
                file("played"),
                same.clone(),
                file("next"),
                same,
                file("last"),
            ]
            .into_iter()
            .collect(),
            1,
        );
        let ids: Vec<u64> = q.entries.iter().map(|e| e.id).collect();
        let active = ids[1];
        let still_active = |q: &Queue| {
            assert_eq!(q.current_entry().map(|e| e.id), Some(active));
        };
        // A delayed request naming the active entry removes only the others.
        assert!(q.remove(&[active, ids[2]]));
        still_active(&q);
        assert_eq!(titles(&q), ["played", "same", "same", "last"]);
        // The duplicate recording is a separate occurrence.
        assert!(q.remove(&[ids[3]]));
        assert_eq!(titles(&q), ["played", "same", "last"]);
        assert!(q.remove(&[ids[0]]));
        assert_eq!(q.current, Some(0));
        still_active(&q);
        q.set_shuffle(true);
        q.set_repeat(Repeat::One);
        q.move_upcoming(active, None);
        q.schedule_next(active);
        still_active(&q);
        q.clear_history();
        q.clear_upcoming();
        still_active(&q);
        assert!(q.upcoming().is_empty());
        assert_eq!(q.next_index(true), Some(0), "Repeat One replays it");
        q.set_repeat(Repeat::Off);
        assert_eq!(q.next_index(true), None, "an empty queue ends playback");
    }

    #[test]
    fn an_idle_sequence_keeps_history_out_of_upcoming() {
        let mut q = Queue::default();
        q.replace([file("played"), file("stopped")].into_iter().collect(), 1);
        let stopped = q.current_entry().unwrap().id;
        q.set_current(None);
        assert!(q.remove(&[stopped]));
        let mut q: Queue = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        q.validate().unwrap();
        assert_eq!(q.history()[0].track.title, "played");
        assert!(q.upcoming().is_empty());
        assert_eq!(q.next_index(false), None);
        q.schedule_next(q.history()[0].id);
        assert_eq!(q.history().len(), 1);
        assert_eq!(q.next_index(false), Some(1));
        q.clear_history();
        assert_eq!(q.next_index(false), Some(0));
        assert_eq!(titles(&q), ["played"]);
    }

    #[test]
    fn replaying_history_next_copies_an_occurrence_and_keeps_it_first_after_shuffle() {
        let recording = Arc::new(file("repeat"));
        let selection = Selection::from_indexed(
            [recording.clone(), Arc::new(file("current")), recording].into(),
            Some((7, vec![40, 41, 42])),
        )
        .named(Some("Repeats".into()));
        let mut q = Queue::default();
        q.replace(selection, 1);
        let original = q.history()[0].clone();
        let current = q.current_entry().unwrap().id;
        let upcoming = q.upcoming()[0].id;
        q.set_shuffle(true);
        q.schedule_next(original.id);
        assert_eq!(q.history(), std::slice::from_ref(&original));
        assert_ne!(q.upcoming()[0].id, original.id);
        assert_eq!(
            q.upcoming()[0].playlist_occurrence,
            original.playlist_occurrence
        );
        q.schedule_next(upcoming);
        q.set_shuffle(false);
        assert_eq!(q.upcoming()[0].id, upcoming);
        assert_eq!(q.current_entry().unwrap().id, current);
        let restored: Queue = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
        assert_eq!(restored.entries, q.entries);
        assert_eq!(restored.source.as_deref(), Some("Repeats"));
        assert_eq!(
            restored.upcoming()[0].playlist_occurrence.unwrap().entry,
            42
        );
    }

    #[test]
    fn repeat_modes() {
        let mut q = Queue::default();
        q.replace([file("1"), file("2"), file("3")].into_iter().collect(), 2);
        assert_eq!(q.next_index(true), None);
        q.set_repeat(Repeat::All);
        assert_eq!(q.next_index(true), Some(0));
        q.set_current(Some(1));
        q.set_repeat(Repeat::One);
        assert_eq!(q.next_index(true), Some(1), "a finished track plays again");
        assert_eq!(
            q.next_index(false),
            Some(2),
            "Next still skips with Repeat One"
        );
        q.set_current(Some(0));
        q.set_repeat(Repeat::Off);
        assert_eq!(q.previous_index(), None);
        q.set_repeat(Repeat::All);
        assert_eq!(q.previous_index(), Some(2));
    }

    #[test]
    fn shuffle_only_reorders_upcoming_and_restores() {
        let mut q = Queue::default();
        let tracks: Vec<Track> = (0..30).map(|i| file(&i.to_string())).collect();
        q.replace(tracks.into_iter().collect(), 4);
        let before = titles(&q);
        q.set_shuffle(true);
        assert_eq!(titles(&q)[..5], before[..5]);
        let mut sorted = titles(&q);
        sorted.sort();
        let mut expected = before.clone();
        expected.sort();
        assert_eq!(sorted, expected);
        q.add(&[file("late")].into_iter().collect());
        q.set_shuffle(false);
        let mut restored = before.clone();
        restored.push("late".into());
        assert_eq!(titles(&q), restored);
    }

    #[test]
    fn continuing_promotes_one_occurrence_and_preserves_manual_repeats() {
        let cd: Vec<_> = (1..=4).map(|n| Arc::new(chapter("A", n))).collect();
        let extra = Arc::new(file("extra"));
        let mut q = Queue::default();
        q.replace(cd[..2].iter().cloned().collect(), 1);
        let current = q.current_entry().unwrap().id;
        let ids = q.add(
            &[extra.clone(), cd[3].clone(), extra, cd[3].clone()]
                .into_iter()
                .collect(),
        );
        let history = q.history().to_vec();
        q.set_repeat(Repeat::One);
        assert!(q.continue_disc(current, &cd, DiscContext::Inferred));
        assert_eq!(
            titles(&q),
            ["A 1", "A 2", "A 3", "A 4", "extra", "extra", "A 4"]
        );
        assert_eq!(q.history(), history);
        assert_eq!(q.current_entry().unwrap().id, current);
        assert_eq!(
            q.upcoming()[1..].iter().map(|e| e.id).collect::<Vec<_>>(),
            [ids[1], ids[0], ids[2], ids[3]]
        );
        assert_eq!(q.repeat, Repeat::Off);
        let revision = q.revision;
        assert!(!q.continue_disc(current, &cd, DiscContext::Inferred));
        assert!(!q.continue_disc(ids[0], &cd, DiscContext::Inferred));
        assert_eq!(q.revision, revision);
        q.set_shuffle(true);
        assert!(q.continue_disc(current, &cd, DiscContext::Inferred));
        assert!(!q.shuffle);
        assert_eq!(q.upcoming()[0].track, cd[2]);
        assert_eq!(q.upcoming()[1].track, cd[3]);
        q.validate().unwrap();
    }

    #[test]
    fn survives_a_round_trip() {
        let mut q = Queue::default();
        q.replace([chapter("A", 1), file("2")].into_iter().collect(), 0);
        q.set_repeat(Repeat::All);
        let json = serde_json::to_string(&q).unwrap();
        let back: Queue = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entries, q.entries);
        assert_eq!((back.current, back.repeat), (Some(0), Repeat::All));
        let mut back = back;
        let ids = back.add(&[file("3")].into_iter().collect());
        assert!(
            ids[0] > q.entries[1].id,
            "identities stay unique after restore"
        );
    }
}
