//! Immutable membership of indexed folders under the active filter. Both the
//! catalog scan and page lookups run on a worker. Resident nodes carry one bit.
use super::{Branch, Item, RootId};
use crate::store::{Store, path_key};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use tempfile::TempPath;

pub(super) struct Descendants {
    pub(super) query: String,
    file: TempPath,
}

impl Descendants {
    pub(super) fn build(
        store: &Store,
        root: Option<RootId>,
        query: String,
    ) -> anyhow::Result<Self> {
        let file = tempfile::NamedTempFile::new()?.into_temp_path();
        let db = Connection::open(&file)?;
        db.execute_batch(
            "PRAGMA journal_mode=OFF; PRAGMA cache_size=-2048;
            CREATE TABLE folders(path TEXT PRIMARY KEY) WITHOUT ROWID; BEGIN",
        )?;
        {
            let mut insert = db.prepare("INSERT OR IGNORE INTO folders VALUES(?)")?;
            store.visit_matching_folders(root, &query, |folder| {
                for ancestor in folder.ancestors() {
                    if insert.execute([path_key(ancestor)])? == 0 {
                        break;
                    }
                }
                Ok(())
            })?;
        }
        db.execute_batch("COMMIT")?;
        Ok(Self { query, file })
    }

    pub(super) fn annotate(&self, mut page: Branch) -> anyhow::Result<Branch> {
        let db = Connection::open_with_flags(&self.file, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut stmt = db.prepare("SELECT 1 FROM folders WHERE path=?")?;
        let mut contains = |p: &Path| stmt.exists([path_key(p)]);
        for node in page
            .pages
            .iter_mut()
            .filter_map(|p| p.nodes.as_mut())
            .flatten()
        {
            node.has_matching_descendant = match &node.item {
                Item::Collection { path, .. } | Item::Folder { path, .. } => contains(path)?,
                Item::Disc { disc, .. } => contains(&disc.folder)?,
                Item::Release { release, .. } => {
                    let mut matched = false;
                    for disc in &release.discs {
                        if contains(&disc.folder)? {
                            matched = true;
                            break;
                        }
                    }
                    matched
                }
                Item::Track { .. } => false,
            };
        }
        Ok(page)
    }
}
