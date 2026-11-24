//! The listening queue: what is playing, what played before it in this
//! session, and what plays next.
//!
//! The queue holds its own copies of the tracks. Browsing, sorting a table or
//! editing a folder never changes it, and changing the queue never changes an
//! album or a playlist. A CUE chapter is an entry like any other file.
use crate::library::{DiscContext, Track};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub track: Arc<Track>,
    #[serde(default)]
    pub disc_context: DiscContext,
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
    pub shuffle: bool,
    pub repeat: Repeat,
    /// The order of upcoming entries before shuffle was switched on.
    unshuffled: Arc<Vec<u64>>,
    next_id: u64,
    #[serde(skip)]
    pub revision: u64,
}

impl Queue {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.current.is_none_or(|i| i < self.entries.len()),
            "Queue cursor is outside its entries"
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
        }
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
        self.current.map_or(0, |c| c + 1)
    }
    pub fn upcoming(&self) -> &[Entry] {
        &self.entries[self.upcoming_start().min(self.entries.len())..]
    }
    pub fn history(&self) -> &[Entry] {
        &self.entries[..self.current.unwrap_or(0).min(self.entries.len())]
    }

    /// Starts a new queue from a table, beginning at `start`. The tracks before
    /// it become history, so Previous moves back through the album.
    pub fn replace(&mut self, tracks: &[impl Clone + Into<Arc<Track>>], start: usize) {
        self.entries = Arc::new(tracks.iter().cloned().map(|t| self.entry(t)).collect());
        self.current = (start < self.entries.len()).then_some(start);
        self.unshuffled = Arc::default();
        if self.shuffle {
            self.shuffle_upcoming();
        }
        self.touch();
    }
    /// Inserts tracks right after the current entry, in the order given.
    pub fn play_next(&mut self, tracks: &[impl Clone + Into<Arc<Track>>]) -> Vec<u64> {
        let at = self.upcoming_start().min(self.entries.len());
        let new: Vec<Entry> = tracks.iter().cloned().map(|t| self.entry(t)).collect();
        let ids = new.iter().map(|e| e.id).collect::<Vec<_>>();
        Arc::make_mut(&mut self.entries).splice(at..at, new);
        if self.shuffle {
            Arc::make_mut(&mut self.unshuffled).splice(0..0, ids.iter().copied());
        }
        self.touch();
        ids
    }
    pub fn add(&mut self, tracks: &[impl Clone + Into<Arc<Track>>]) -> Vec<u64> {
        let new: Vec<Entry> = tracks.iter().cloned().map(|t| self.entry(t)).collect();
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
    /// Removes entries. Returns true when the current entry was among them;
    /// the entry that followed it then becomes current.
    pub fn remove(&mut self, ids: &[u64]) -> bool {
        let removed_current = self.current_entry().is_some_and(|e| ids.contains(&e.id));
        let current = self.current.map(|c| {
            c - self.entries[..c]
                .iter()
                .filter(|e| ids.contains(&e.id))
                .count()
        });
        Arc::make_mut(&mut self.entries).retain(|e| !ids.contains(&e.id));
        // When the current entry went, the one that followed slid into its slot.
        self.current = current.filter(|&c| c < self.entries.len());
        Arc::make_mut(&mut self.unshuffled).retain(|id| !ids.contains(id));
        self.touch();
        removed_current
    }
    /// Moves an entry to the position `to` in the full list (history included).
    pub fn move_entry(&mut self, id: u64, to: usize) {
        let Some(from) = self.index_of(id) else {
            return;
        };
        let to = to.min(self.entries.len());
        if to == from || to == from + 1 {
            return;
        }
        let current_id = self.current_entry().map(|e| e.id);
        let entry = Arc::make_mut(&mut self.entries).remove(from);
        let to = if to > from { to - 1 } else { to };
        Arc::make_mut(&mut self.entries).insert(to, entry);
        self.current = current_id.and_then(|id| self.index_of(id));
        self.touch();
    }
    pub fn clear_upcoming(&mut self) {
        let start = self.upcoming_start().min(self.entries.len());
        Arc::make_mut(&mut self.entries).truncate(start);
        self.unshuffled = Arc::default();
        self.touch();
    }
    pub fn clear_history(&mut self) {
        if let Some(c) = self.current {
            Arc::make_mut(&mut self.entries).drain(..c);
            self.current = Some(0);
            self.touch();
        }
    }
    pub fn set_current(&mut self, index: Option<usize>) {
        let index = index.filter(|&i| i < self.entries.len());
        if self.current != index {
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
            return Some(0);
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
        q.add(&[chapter("A", 7), file("single"), chapter("B", 11)]);
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
        q.replace(&[file("1"), file("2"), file("3")], 1);
        q.play_next(&[file("x"), file("y")]);
        q.add(&[file("z")]);
        assert_eq!(titles(&q), ["1", "2", "x", "y", "3", "z"]);
        assert_eq!(q.current, Some(1));
        assert_eq!(q.history().len(), 1);
        assert_eq!(q.upcoming().len(), 4);
    }

    #[test]
    fn reordering_removing_and_clearing() {
        let mut q = Queue::default();
        q.replace(&[file("1"), file("2"), file("3"), file("4")], 1);
        let id4 = q.entries[3].id;
        q.move_entry(id4, 2);
        assert_eq!(titles(&q), ["1", "2", "4", "3"]);
        // Moving history around keeps the same entry current.
        let id1 = q.entries[0].id;
        q.move_entry(id1, 4);
        assert_eq!(titles(&q), ["2", "4", "3", "1"]);
        assert_eq!(q.current_entry().unwrap().track.title, "2");
        let id3 = q.entries[2].id;
        assert!(!q.remove(&[id3]));
        assert_eq!(titles(&q), ["2", "4", "1"]);
        let current = q.current_entry().unwrap().id;
        assert!(q.remove(&[current]));
        assert_eq!(q.current_entry().unwrap().track.title, "4");
        q.clear_upcoming();
        assert_eq!(titles(&q), ["4"]);
    }

    #[test]
    fn repeat_modes() {
        let mut q = Queue::default();
        q.replace(&[file("1"), file("2"), file("3")], 2);
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
        q.replace(&tracks, 4);
        let before = titles(&q);
        q.set_shuffle(true);
        assert_eq!(titles(&q)[..5], before[..5]);
        let mut sorted = titles(&q);
        sorted.sort();
        let mut expected = before.clone();
        expected.sort();
        assert_eq!(sorted, expected);
        q.add(&[file("late")]);
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
        q.replace(&cd[..2], 1);
        let current = q.current_entry().unwrap().id;
        let ids = q.add(&[extra.clone(), cd[3].clone(), extra, cd[3].clone()]);
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
        q.replace(&[chapter("A", 1), file("2")], 0);
        q.set_repeat(Repeat::All);
        let json = serde_json::to_string(&q).unwrap();
        let back: Queue = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entries, q.entries);
        assert_eq!((back.current, back.repeat), (Some(0), Repeat::All));
        let mut back = back;
        let ids = back.add(&[file("3")]);
        assert!(
            ids[0] > q.entries[1].id,
            "identities stay unique after restore"
        );
    }
}
