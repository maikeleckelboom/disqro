//! Browser history and exact-track navigation between folders, indexed CDs and the player.
use super::*;

/// A history entry owns navigation state only. Folder CD pages can project
/// the open scan without replacing its root or starting another walker.
pub(super) struct Visit {
    location: Location,
    disc: Option<DiscId>,
    albums: bool,
    query: String,
    sort: Option<(Field, ColumnSort)>,
    bookmark: Bookmark,
}
pub(super) struct Bookmark {
    selected: Option<RowKey>,
    marked: Option<crate::table::Selection>,
    offset: Point<Pixels>,
}

impl Browser {
    fn visit(&self, cx: &App) -> Visit {
        let table = self.table.read(cx);
        let bookmark = if self.listing_discs() {
            let t = self.discs.read(cx);
            Bookmark {
                marked: None,
                selected: t
                    .selected_row()
                    .and_then(|i| t.delegate().items.get(i))
                    .map(|d| RowKey::Disc(d.id.clone())),
                offset: t.vertical_scroll_handle.0.borrow().base_handle.offset(),
            }
        } else {
            Bookmark {
                marked: table.delegate().marked_selection(),
                selected: table
                    .selected_row()
                    .and_then(|i| table.delegate().selected_key(i)),
                offset: table.vertical_scroll_handle.0.borrow().base_handle.offset(),
            }
        };
        Visit {
            location: self.location.clone(),
            disc: self.disc.clone(),
            albums: self.prefs.albums,
            query: self.query.clone(),
            sort: table.delegate().sort,
            bookmark,
        }
    }
    pub(super) fn cancel_reveal(&mut self) {
        self.navigation += 1;
        self.bookmark = None;
        self.reveal_key = None;
    }
    pub(super) fn push_visit(&mut self, cx: &App) {
        self.cancel_reveal();
        self.back.push(self.visit(cx));
        if self.back.len() > HISTORY {
            self.back.remove(0);
        }
        self.forward.clear();
    }
    pub(super) fn remember(&mut self, next: &Location, cx: &App) {
        let same = match (&self.location, next) {
            (Location::Folder(a), Location::Folder(b)) => {
                a.as_os_str().is_empty() || (self.disc.is_none() && crate::places::same_path(a, b))
            }
            (a, b) => a == b,
        };
        if !same {
            self.push_visit(cx);
        } else {
            self.cancel_reveal();
        }
    }
    pub(super) fn restore_bookmark(&mut self, cx: &mut App) {
        let Some(mark) = self.bookmark.take() else {
            return;
        };
        if self.listing_discs() {
            self.discs.update(cx, |t, cx| {
                let ix = mark.selected.and_then(|k| {
                    t.delegate()
                        .items
                        .iter()
                        .position(|d| k == RowKey::Disc(d.id.clone()))
                });
                reselect(t, ix, cx);
                t.vertical_scroll_handle
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(mark.offset);
            });
        } else {
            self.table.update(cx, |t, cx| {
                let ix = mark.selected.and_then(|k| t.delegate().position(&k));
                reselect(t, ix, cx);
                if let Some(marked) = mark.marked {
                    t.delegate_mut().restore_marks(marked);
                }
                t.vertical_scroll_handle
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(mark.offset);
            });
        }
    }
    fn restore_visit(&mut self, visit: Visit, w: &mut Window, cx: &mut Context<Self>) {
        self.cancel_reveal();
        let same_folder = visit.location.is_folder() && self.location == visit.location;
        self.prefs.albums = visit.albums;
        if !same_folder {
            self.enter(visit.location, w, cx);
        }
        self.disc = visit.disc;
        self.reset_table(cx);
        self.table
            .update(cx, |t, _| t.delegate_mut().sort = visit.sort);
        self.query = visit.query;
        self.search
            .update(cx, |s, cx| s.set_value(self.query.clone(), w, cx));
        self.bookmark = Some(visit.bookmark);
        self.filter(cx);
        if self.location.is_folder() && !self.busy {
            self.restore_bookmark(cx);
        }
        self.list_focus(cx).focus(w, cx);
        cx.emit(ViewChanged);
        cx.notify();
    }
    pub(super) fn enter(
        &mut self,
        location: Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match location {
            Location::Folder(path) if self.folder_ready && self.path == path => {
                self.location = Location::Folder(path.clone());
                self.disc = None;
                self.notice = None;
                self.reset_table(cx);
                self.clear_search(window, cx);
                self.folders.update(cx, |f, cx| f.show(&path, false, cx));
                self.filter(cx);
                self.list_focus(cx).focus(window, cx);
                cx.emit(ViewChanged);
                cx.notify();
            }
            Location::Folder(path) => self.load(path, None, window, cx),
            location => {
                if self.location != location {
                    self.disc = None;
                    self.search.update(cx, |s, cx| s.set_value("", window, cx));
                    self.query.clear();
                    self.table.update(cx, |t, _| scroll_to_top(t));
                    self.discs.update(cx, |t, _| scroll_to_top(t));
                }
                self.task.take();
                self.generation += 1;
                self.busy = false;
                self.notice = None;
                self.location = location;
                self.count = 0;
                self.duration = None;
                // The tree keeps the last folder in view without marking it
                // as the current location.
                let path = self.path.clone();
                self.folders.update(cx, |f, cx| {
                    if !path.as_os_str().is_empty() && f.is_empty() {
                        f.show(&path, false, cx);
                    }
                    f.set_highlight(false, cx);
                });
                cx.emit(ViewChanged);
                self.filter(cx);
                self.list_focus(cx).focus(window, cx);
                cx.notify();
            }
        }
    }
    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(l) = self.back.pop() {
            self.forward.push(self.visit(cx));
            self.restore_visit(l, window, cx);
        }
    }
    pub fn forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(l) = self.forward.pop() {
            self.back.push(self.visit(cx));
            self.restore_visit(l, window, cx);
        }
    }
    pub fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disc.is_some() {
            if self
                .back
                .last()
                .is_some_and(|v| v.location == self.location && v.disc.is_none())
            {
                self.back(window, cx);
            } else {
                self.push_visit(cx);
                self.project_disc(None, window, cx);
            }
            return;
        }
        match &self.location {
            Location::Folder(_) => {
                if let Some(parent) = self.path.parent().map(Path::to_owned) {
                    self.open(parent, window, cx);
                }
            }
            Location::Album { root, .. } => self.show(Location::Library(*root), window, cx),
            _ => {}
        }
    }
    pub fn can_go_up(&self) -> bool {
        match self.location {
            Location::Folder(_) => self.disc.is_some() || self.path.parent().is_some(),
            Location::Album { .. } => true,
            _ => self.disc.is_some(),
        }
    }
    pub(super) fn select_revealed(&mut self, cx: &mut App) -> bool {
        let Some(key) = &self.reveal_key else {
            return false;
        };
        let Some(i) = self.table.read(cx).delegate().position(key) else {
            return false;
        };
        self.table.update(cx, |t, cx| {
            t.set_selected_row(i, cx);
            t.scroll_to_row(i, cx);
        });
        self.reveal_key = None;
        true
    }

    pub fn locate(
        &mut self,
        track: Arc<Track>,
        dest: Destination,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_reveal();
        // Reuse a collection already on screen, including its unfinished scan.
        if track.library_id.is_none()
            && self.location.is_folder()
            && self
                .programs
                .iter()
                .any(|d| d.id.contains(&track) && d.tracks.iter().any(|t| t.same_item(&track)))
            && (dest == Destination::Disc || self.can_list_discs())
        {
            self.reveal_folder(track, dest, self.path.clone(), w, cx);
            return;
        }
        let revision = self.navigation;
        let lookup = self.catalog.read(cx).handle().map(|store| {
            let target = track.clone();
            store.call(move |s| s.registered_disc(&target))
        });
        cx.spawn_in(w, async move |this, cx| {
            let indexed = match lookup {
                Some(result) => result.await,
                None => Ok(None),
            };
            let _ = this.update_in(cx, |b, w, cx| {
                if b.navigation != revision {
                    return;
                }
                let indexed = match indexed {
                    Ok(disc) => disc,
                    Err(e) if track.library_id.is_some() => {
                        b.notice = Some(format!("{e:#}"));
                        cx.notify();
                        return;
                    }
                    Err(_) => None,
                };
                if let Some(disc) = indexed {
                    let location = match dest {
                        Destination::Disc => Location::Album {
                            root: Some(disc.root),
                            id: disc.album,
                            title: disc.title,
                        },
                        Destination::Collection => Location::Library(Some(disc.root)),
                    };
                    b.remember(&location, cx);
                    if dest == Destination::Collection {
                        b.prefs.albums = false;
                    }
                    b.clear_search(w, cx);
                    b.enter(location, w, cx);
                    b.reveal_key = Some(RowKey::Key(disc.track));
                    // The key listing will reveal even an off-screen, unpaged chapter.
                } else {
                    let anchor = track.disc.as_ref().map_or(&track.source, |d| &d.sheet);
                    let Some(parent) = anchor.parent() else {
                        return;
                    };
                    let folder =
                        if dest == Destination::Collection && b.prefs.includes_subfolders(parent) {
                            b.prefs
                                .subfolders
                                .iter()
                                .filter(|r| r.include && library::within(parent, &r.path))
                                .max_by_key(|r| r.path.components().count())
                                .map(|r| r.path.as_path())
                                .unwrap_or(parent)
                        } else {
                            parent
                        };
                    b.reveal_folder(track.clone(), dest, folder.into(), w, cx);
                }
            });
        })
        .detach();
    }

    fn reveal_folder(
        &mut self,
        track: Arc<Track>,
        dest: Destination,
        folder: PathBuf,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let disc = (dest == Destination::Disc).then(|| DiscId::of(&track));
        let same_folder = self.location == Location::Folder(folder.clone());
        if !same_folder || self.disc != disc || self.listing_discs() || !self.query.is_empty() {
            self.push_visit(cx);
        }
        if dest == Destination::Collection {
            self.prefs.albums = false;
        }
        if !same_folder {
            self.load(folder, None, w, cx);
        }
        self.project_disc(disc, w, cx);
        self.reveal_key = Some(RowKey::Track(track.source.clone(), track.disc.clone()));
        if !self.select_revealed(cx) && !self.busy {
            self.notice = Some("This recording is unavailable.".into());
        }
    }

    pub fn continue_cd(&self, request: crate::queue::Entry, cx: &mut Context<Self>) {
        let crate::queue::Entry {
            id: entry,
            track,
            disc_context: context,
        } = request;
        if !context.allows(&track) {
            return;
        }
        let player = self.table.read(cx).delegate().player.clone();
        if track.library_id.is_none()
            && self.location.is_folder()
            && !self.busy
            && let Some(disc) = self.programs.iter().find(|d| d.id.contains(&track))
        {
            let program = disc.tracks.clone().into();
            player.update(cx, |p, cx| {
                p.command(
                    crate::audio::Command::ContinueDisc(entry, program, context),
                    cx,
                )
            });
            return;
        }
        let catalog = self.catalog.clone();
        cx.spawn(async move |_, cx| {
            let result = Catalog::program(catalog.clone(), track.clone(), context, cx).await;
            match result {
                Ok(tracks) if tracks.iter().any(|t| t.same_item(&track)) => {
                    player.update(cx, |p, cx| {
                        p.command(
                            crate::audio::Command::ContinueDisc(entry, tracks, context),
                            cx,
                        )
                    });
                }
                other => catalog.update(cx, |_, cx| {
                    cx.emit(CatalogEvent::Notice(other.err().map_or_else(
                        || "This CD is unavailable.".into(),
                        |e| format!("{e:#}"),
                    )))
                }),
            }
        })
        .detach();
    }
}
