//! ISO BMFF edit-list timing for the selected audio track. Walk only box
//! headers and the fixed-size fields needed for timing, skipping media data.
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Copy, Clone)]
struct Atom {
    data: u64,
    end: u64,
}
fn bytes<const N: usize>(f: &mut File, a: Atom, offset: u64) -> Result<[u8; N]> {
    let start = a
        .data
        .checked_add(offset)
        .context("Invalid MP4 field offset")?;
    ensure!(
        a.end.saturating_sub(start) >= N as u64,
        "Truncated MP4 field"
    );
    f.seek(SeekFrom::Start(start))?;
    let mut b = [0; N];
    f.read_exact(&mut b)?;
    Ok(b)
}
fn children(f: &mut File, parent: Atom, id: [u8; 4]) -> Result<Vec<Atom>> {
    let mut pos = parent.data;
    let mut found = vec![];
    let mut count = 0;
    while pos < parent.end {
        count += 1;
        ensure!(count <= 65536, "Too many MP4 boxes");
        let rest = Atom {
            data: pos,
            end: parent.end,
        };
        let header = bytes::<8>(f, rest, 0)?;
        let size = u32::from_be_bytes(header[..4].try_into()?);
        let (size, head) = match size {
            0 => (parent.end - pos, 8),
            1 => (u64::from_be_bytes(bytes(f, rest, 8)?), 16),
            n => (u64::from(n), 8),
        };
        ensure!(
            size >= head && size <= parent.end - pos,
            "Invalid MP4 box length"
        );
        if header[4..] == id {
            found.push(Atom {
                data: pos + head,
                end: pos + size,
            });
        }
        pos += size;
    }
    Ok(found)
}
fn one(f: &mut File, parent: Atom, id: [u8; 4]) -> Result<Option<Atom>> {
    let v = children(f, parent, id)?;
    ensure!(v.len() <= 1, "Duplicate MP4 timing box");
    Ok(v.into_iter().next())
}
fn header_field(f: &mut File, a: Atom) -> Result<u32> {
    let offset = match bytes::<1>(f, a, 0)?[0] {
        0 => 12,
        1 => 20,
        _ => bail!("Unsupported MP4 header version"),
    };
    Ok(u32::from_be_bytes(bytes(f, a, offset)?))
}
// A standard one-entry media edit removes encoder priming and trailing padding.
// Complex edits require a segmented timeline; reject them explicitly.
pub fn edit(path: &Path, id: u32, rate: u32) -> Result<Option<(i64, usize)>> {
    let mut f = File::open(path)?;
    let root = Atom {
        data: 0,
        end: f.metadata()?.len(),
    };
    let movie = one(&mut f, root, *b"moov")?.context("Missing MP4 movie")?;
    let mvhd = one(&mut f, movie, *b"mvhd")?.context("Missing MP4 timescale")?;
    let scale = header_field(&mut f, mvhd)?;
    ensure!(scale > 0, "Invalid MP4 timescale");
    for t in children(&mut f, movie, *b"trak")? {
        let Some(header) = one(&mut f, t, *b"tkhd")? else {
            continue;
        };
        if header_field(&mut f, header)? != id {
            continue;
        }
        let Some(edts) = one(&mut f, t, *b"edts")? else {
            return Ok(None);
        };
        let Some(list) = one(&mut f, edts, *b"elst")? else {
            return Ok(None);
        };
        let n = u32::from_be_bytes(bytes(&mut f, list, 4)?);
        if n == 0 {
            return Ok(None);
        }
        ensure!(n == 1, "Multi-part MP4 edits are unsupported");
        let (duration, first, speed) = match bytes::<1>(&mut f, list, 0)?[0] {
            0 => (
                u64::from(u32::from_be_bytes(bytes(&mut f, list, 8)?)),
                i64::from(i32::from_be_bytes(bytes(&mut f, list, 12)?)),
                16,
            ),
            1 => (
                u64::from_be_bytes(bytes(&mut f, list, 8)?),
                i64::from_be_bytes(bytes(&mut f, list, 16)?),
                24,
            ),
            _ => bail!("Unsupported MP4 edit version"),
        };
        ensure!(
            first >= 0 && u32::from_be_bytes(bytes(&mut f, list, speed)?) == 0x10000,
            "Unsupported MP4 edit rate or empty edit"
        );
        let count =
            (u128::from(duration) * u128::from(rate) + u128::from(scale) / 2) / u128::from(scale);
        return Ok(Some((first, usize::try_from(count)?)));
    }
    bail!("MP4 audio track identity could not be resolved")
}
