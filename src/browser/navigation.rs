//! Browser history and exact-track navigation between folders, indexed CDs and the player.
use super::*;

/// A history entry owns navigation state only. Folder CD pages can project
/// the open scan without replacing its root or starting another walker.
pub(super) struct Visit {
    location: Location,
    page: Option<Page>,
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
                    .and_then(|i| t.delegate().rows.get(i))
                    .map(Shelf::key),
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
            page: self.page.clone(),
            albums: self.prefs.albums,
            query: self.query.clone(),
            sort: table.delegate().sort,
            bookmark,
        }
    }
    /// Scrolls a restored view back to where it was, once it has listed.
    pub fn restore_scroll(&mut self, offset: f32) {
        if offset > 0. {
            self.bookmark = Some(Bookmark {
                selected: None,
                marked: None,
                offset: point(px(0.), px(-offset)),
            });
        }
    }
    /// How far the list is scrolled down.
    pub fn scroll_offset(&self, cx: &App) -> f32 {
        let offset = if self.listing_discs() {
            self.discs
                .read(cx)
                .vertical_scroll_handle
                .0
                .borrow()
                .base_handle
                .offset()
        } else {
            self.table
                .read(cx)
                .vertical_scroll_handle
                .0
                .borrow()
                .base_handle
                .offset()
        };
        (-f32::from(offset.y)).max(0.)
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
                a.as_os_str().is_empty() || (self.page.is_none() && crate::places::same_path(a, b))
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
                let ix = mark
                    .selected
                    .and_then(|k| t.delegate().rows.iter().position(|r| r.key() == k));
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
        self.page = visit.page;
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
                self.page = None;
                self.notice = None;
                self.reset_table(cx);
                self.clear_search(window, cx);
                if self.active {
                    self.folders.update(cx, |f, cx| f.show(&path, None, cx));
                }
                self.filter(cx);
                self.list_focus(cx).focus(window, cx);
                cx.emit(ViewChanged);
                cx.notify();
            }
            Location::Folder(path) => self.load(path, None, window, cx),
            location => {
                if self.location != location {
                    self.page = None;
                    self.search.update(cx, |s, cx| s.set_value("", window, cx));
                    self.query.clear();
                    self.table.update(cx, |t, _| scroll_to_top(t));
                    self.discs.update(cx, |t, _| scroll_to_top(t));
                }
                self.task.take();
                self.generation += 1;
                self.busy = false;
                self.notice = None;
                self.membership = None;
                if let Location::Album { id, .. } = location {
                    self.load_membership(id, cx);
                }
                self.location = location;
                self.count = 0;
                self.duration = None;
                // The tree keeps the last folder in view without marking it
                // as the current location.
                let path = self.path.clone();
                if self.active {
                    self.folders.update(cx, |f, cx| {
                        if !path.as_os_str().is_empty() && f.is_empty() {
                            f.show(&path, None, cx);
                        }
                        f.set_highlight(false, cx);
                    });
                }
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
    /// Goes up one level: from a CD to its release, from a release or a CD
    /// to the collection, from a folder to its parent. When the level above
    /// is the previous history entry, Back restores it as it was.
    pub fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let release = self.membership.clone();
        let target = match (&self.page, &self.location) {
            (Some(Page::Disc(_)), location) if let Some(m) = &release => {
                (location.clone(), Some(Page::Release(m.key.clone())))
            }
            (Some(_), location) => (location.clone(), None),
            (None, Location::Album { root, .. }) => match &release {
                Some(m) => match m.key {
                    DiscId::Library(id) => (
                        Location::Release {
                            root: *root,
                            id,
                            title: m.title.clone(),
                        },
                        None,
                    ),
                    _ => (Location::Library(*root), None),
                },
                None => (Location::Library(*root), None),
            },
            (None, Location::Release { root, .. }) => (Location::Library(*root), None),
            (None, Location::Folder(_)) => {
                if let Some(parent) = self.path.parent().map(Path::to_owned) {
                    self.open(parent, window, cx);
                }
                return;
            }
            _ => return,
        };
        if self
            .back
            .last()
            .is_some_and(|v| v.location == target.0 && v.page == target.1)
        {
            self.back(window, cx);
        } else if target.0 == self.location {
            self.push_visit(cx);
            self.project_page(target.1, window, cx);
        } else {
            self.show(target.0, window, cx);
        }
    }
    /// Returns from a CD or release page of a folder to its collection.
    pub fn show_collection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page.is_none() {
            return;
        }
        if self
            .back
            .last()
            .is_some_and(|v| v.location == self.location && v.page.is_none())
        {
            self.back(window, cx);
        } else {
            self.push_visit(cx);
            self.project_page(None, window, cx);
        }
    }
    pub fn can_go_up(&self) -> bool {
        match self.location {
            Location::Folder(_) => self.page.is_some() || self.path.parent().is_some(),
            Location::Album { .. } | Location::Release { .. } => true,
            _ => self.page.is_some(),
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
        let page = (dest == Destination::Disc).then(|| Page::Disc(DiscId::of(&track)));
        let same_folder = self.location == Location::Folder(folder.clone());
        if !same_folder || self.page != page || self.listing_discs() || !self.query.is_empty() {
            self.push_visit(cx);
        }
        if dest == Destination::Collection {
            self.prefs.albums = false;
        }
        if !same_folder {
            self.load(folder, None, w, cx);
        }
        self.project_page(page, w, cx);
        self.reveal_key = Some(RowKey::Track(track.source.clone(), track.disc.clone()));
        if !self.select_revealed(cx) && !self.busy {
            self.notice = Some("This recording is unavailable.".into());
        }
    }

    pub fn continue_cd(&self, request: crate::queue::Entry, cx: &mut Context<Self>) {
        let request = match self.continue_from_folder(request, cx) {
            Some(request) => request,
            None => return,
        };
        let player = self.table.read(cx).delegate().player.clone();
        continue_cd(&self.catalog, &player, request, cx);
    }
    /// Continues a CD whose folder this browser has open, without asking the
    /// library. Hands the request back when it cannot.
    fn continue_from_folder(
        &self,
        request: crate::queue::Entry,
        cx: &mut App,
    ) -> Option<crate::queue::Entry> {
        if !request.disc_context.allows(&request.track) {
            return None;
        }
        let player = self.table.read(cx).delegate().player.clone();
        if request.track.library_id.is_none()
            && self.location.is_folder()
            && !self.busy
            && let Some(disc) = self.programs.iter().find(|d| d.id.contains(&request.track))
        {
            let program = disc.tracks.clone().into();
            let (entry, context) = (request.id, request.disc_context);
            player.update(cx, |p, cx| {
                p.command(
                    crate::audio::Command::ContinueDisc(entry, program, context),
                    cx,
                )
            });
            return None;
        }
        Some(request)
    }
}

/// Continues the CD of a queue entry from the program Disqro reads for it,
/// for when no open browser lists that CD.
pub fn continue_cd(
    catalog: &Entity<Catalog>,
    player: &Entity<crate::player::Player>,
    request: crate::queue::Entry,
    cx: &mut App,
) {
    let crate::queue::Entry {
        id: entry,
        track,
        disc_context: context,
    } = request;
    if !context.allows(&track) {
        return;
    }
    let (catalog, player) = (catalog.clone(), player.clone());
    cx.spawn(async move |cx| {
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
