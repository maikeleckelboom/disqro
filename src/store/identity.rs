//! Bounded evidence for music files. Metadata may be rewritten independently
//! of audio. Compare encoded payload where its boundaries are recognizable.
use crate::cache::Stamp;
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Clone, Debug)]
pub(super) struct Evidence {
    pub volume: Option<i64>,
    pub index: Option<i64>,
    /// V1 prefix digest, also used to narrow relocation candidates.
    pub digest: i64,
    pub sample: i64,
    /// Whole payload or a FLAC stream checksum corroborated by frame samples.
    pub strong: bool,
}

/// Stored alongside the track so detached playlist sources can be checked
/// on a background worker without holding the SQLite connection.
#[derive(Clone, Debug)]
pub(crate) struct Revision {
    size: i64,
    modified: i64,
    volume: Option<i64>,
    index: Option<i64>,
    digest: Option<i64>,
    sample: Option<i64>,
}
impl Revision {
    pub(super) fn columns(row: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            size: row.get(first)?,
            modified: row.get(first + 1)?,
            volume: row.get(first + 2)?,
            index: row.get(first + 3)?,
            digest: row.get(first + 4)?,
            sample: row.get(first + 5)?,
        })
    }
    pub(super) fn matches(&self, size: i64, modified: i64, e: &Evidence) -> bool {
        let same_object = self.volume.is_some()
            && self.index.is_some_and(|id| id != 0)
            && (self.volume, self.index) == (e.volume, e.index);
        let same_revision =
            self.size == size && self.modified == modified && self.digest == Some(e.digest);
        match self.sample {
            // Schema 1 only sampled the prefix. Establish the new baseline
            // once for an unchanged revision of the same native object.
            None => same_object && same_revision,
            Some(sample) => {
                sample == e.sample
                    && (e.strong
                        || same_object
                        || (e.index.unwrap_or(0) == 0
                            && self.index.unwrap_or(0) == 0
                            && same_revision))
            }
        }
    }
}

fn digest(bytes: &[u8], size: u64) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in size.to_le_bytes().iter().chain(bytes) {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash as i64
}

impl Evidence {
    pub fn read(path: &Path, sheet: bool, stamp: &Stamp) -> Option<Self> {
        let mut file = fs::File::open(path).ok()?;
        let (volume, index) = file_identity(&file);
        let size = stamp.bytes as u64;
        let mut head = vec![0; size.min(if sheet { 1 << 20 } else { 64 << 10 }) as usize];
        file.read_exact(&mut head).ok()?;
        let prefix = digest(&head, size);
        let (sample, strong) = if sheet {
            (prefix, true)
        } else {
            let (start, end, mut bytes, checksum) = payload(&mut file, &head, size)?;
            const WINDOW: u64 = 8192;
            let len = end.checked_sub(start)?;
            let mut read = |offset, n| -> Option<()> {
                let old = bytes.len();
                bytes.resize(old + n as usize, 0);
                if offset + n <= head.len() as u64 {
                    bytes[old..].copy_from_slice(&head[offset as usize..(offset + n) as usize]);
                } else {
                    file.seek(SeekFrom::Start(offset)).ok()?;
                    file.read_exact(&mut bytes[old..]).ok()?;
                }
                Some(())
            };
            if len <= 3 * WINDOW {
                read(start, len)?;
            } else {
                for offset in [0, (len - WINDOW) / 2, len - WINDOW] {
                    read(start + offset, WINDOW)?;
                }
            }
            (digest(&bytes, len), checksum || len <= 3 * WINDOW)
        };
        (Stamp::read(path).as_ref() == Some(stamp)).then_some(Self {
            volume,
            index,
            digest: prefix,
            sample,
            strong,
        })
    }
}

/// Return the byte range and format data that determine the recording.
/// Skip tags by seeking over their declared lengths. Cap header traversal.
fn payload(file: &mut fs::File, head: &[u8], size: u64) -> Option<(u64, u64, Vec<u8>, bool)> {
    let raw = || Some((0, size, Vec::new(), false));
    if head.starts_with(b"fLaC")
        && head.len() >= 42
        && head[4] & 127 == 0
        && head[5..8] == [0, 0, 34]
    {
        let mut pos = 4;
        for _ in 0..128 {
            file.seek(SeekFrom::Start(pos)).ok()?;
            let mut h = [0; 4];
            file.read_exact(&mut h).ok()?;
            pos += 4 + u64::from(u32::from_be_bytes([0, h[1], h[2], h[3]]));
            if pos > size {
                return raw();
            }
            if h[0] & 128 != 0 {
                return Some((
                    pos,
                    size,
                    head[8..42].to_vec(),
                    head[26..42].iter().any(|b| *b != 0),
                ));
            }
        }
    } else if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WAVE") {
        let mut pos = 12;
        let mut format = Vec::new();
        for _ in 0..128 {
            if pos + 8 > size {
                break;
            }
            file.seek(SeekFrom::Start(pos)).ok()?;
            let mut h = [0; 8];
            file.read_exact(&mut h).ok()?;
            let len = u64::from(u32::from_le_bytes(h[4..].try_into().ok()?));
            pos += 8;
            if pos + len > size {
                break;
            }
            if &h[..4] == b"fmt " && (16..=1024).contains(&len) {
                format.resize(len as usize, 0);
                file.read_exact(&mut format).ok()?;
            } else if &h[..4] == b"data" && !format.is_empty() {
                return Some((pos, pos + len, format, false));
            }
            pos += len + len % 2;
        }
    } else if head.starts_with(b"ID3") && head.len() >= 10 && head[6..10].iter().all(|b| *b < 128) {
        let len = head[6..10]
            .iter()
            .fold(0u64, |v, b| v * 128 + u64::from(*b));
        let start = 10
            + len
            + if head[3] == 4 && head[5] & 16 != 0 {
                10
            } else {
                0
            };
        if start < size {
            return Some((start, size, Vec::new(), false));
        }
    }
    raw()
}

#[cfg(windows)]
fn file_identity(file: &fs::File) -> (Option<i64>, Option<i64>) {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle},
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle belongs to `file`, which outlives the call.
    match unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) } {
        Ok(()) => (
            Some(i64::from(info.dwVolumeSerialNumber)),
            Some(((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)) as i64),
        ),
        Err(_) => (None, None),
    }
}
#[cfg(unix)]
fn file_identity(file: &fs::File) -> (Option<i64>, Option<i64>) {
    use std::os::unix::fs::MetadataExt;
    file.metadata().map_or((None, None), |m| {
        (Some(m.dev() as i64), Some(m.ino() as i64))
    })
}
