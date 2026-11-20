use crate::library::{DiscTrack, Track, is_audio};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

fn time(s: &str) -> Result<u64> {
    let mut fields = s.split(':');
    let mut part = || -> Result<u64> { Ok(fields.next().context("Incomplete CUE time")?.parse()?) };
    let (m, s, f) = (part()?, part()?, part()?);
    ensure!(
        fields.next().is_none() && m <= 10080 && s < 60 && f < 75,
        "Invalid CUE time"
    );
    Ok((m * 60 + s) * 75 + f)
}
fn value(s: &str) -> Result<(&str, &str)> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('"') {
        rest.split_once('"').context("Unclosed CUE string")
    } else {
        Ok(s.split_once(char::is_whitespace).unwrap_or((s, "")))
    }
}
fn title(s: &str) -> Result<String> {
    Ok(if s.trim().starts_with('"') {
        value(s)?.0
    } else {
        s.trim()
    }
    .to_owned())
}
pub fn read(sheet: &Path) -> Result<Vec<Track>> {
    let mut bytes = Vec::new();
    File::open(sheet)?.take(1_048_577).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1_048_576, "Sheet exceeds 1 MiB");
    let text = match std::str::from_utf8(&bytes) {
        Ok(s) => std::borrow::Cow::Borrowed(s),
        Err(_) => {
            encoding_rs::WINDOWS_1252
                .decode_without_bom_handling(&bytes)
                .0
        }
    };
    parse(sheet, &text)
}
fn parse(sheet: &Path, text: &str) -> Result<Vec<Track>> {
    let mut source: Option<PathBuf> = None;
    let (mut album, mut artist) = (String::new(), String::new());
    let mut tracks: Vec<Track> = Vec::new();
    let folder_disc = crate::library::folder_disc_number(sheet);
    let mut year = None;
    let mut in_track = false;
    for line in text.trim_start_matches('\u{feff}').lines() {
        let (key, args) = line
            .trim()
            .split_once(char::is_whitespace)
            .unwrap_or((line.trim(), ""));
        let args = args.trim();
        match key.to_ascii_uppercase().as_str() {
            "REM" if !in_track => {
                if let Some((k, v)) = args.split_once(char::is_whitespace)
                    && k.eq_ignore_ascii_case("DATE")
                {
                    year = v.trim().trim_matches('"').parse().ok();
                }
            }
            "FILE" => {
                let (file, _) = value(args)?;
                let p = sheet.parent().unwrap_or(Path::new("")).join(file);
                ensure!(is_audio(&p), "Unsupported CUE source");
                source = Some(p);
                in_track = false;
            }
            "TRACK" => {
                let (n, kind) = value(args)?;
                ensure!(
                    kind.trim().eq_ignore_ascii_case("audio"),
                    "Mixed-mode CUE is unsupported"
                );
                let n: u8 = n.parse()?;
                ensure!(
                    (1..=99).contains(&n)
                        && tracks
                            .last()
                            .and_then(|t| t.disc.as_ref())
                            .is_none_or(|d| d.number < n),
                    "Invalid track order"
                );
                let mut t = Track::file(source.clone().context("TRACK before FILE")?);
                t.title = format!("Track {n:02}");
                t.year = year;
                t.disc_number = folder_disc;
                t.album = album.clone();
                t.artist = artist.clone();
                t.disc = Some(DiscTrack {
                    sheet: sheet.into(),
                    number: n,
                    start: u64::MAX,
                    end: None,
                });
                tracks.push(t);
                in_track = true;
            }
            "TITLE" if in_track => tracks.last_mut().unwrap().title = title(args)?,
            "PERFORMER" if in_track => tracks.last_mut().unwrap().artist = title(args)?,
            "TITLE" => album = title(args)?,
            "PERFORMER" => artist = title(args)?,
            "INDEX" => {
                let (ix, val) = value(args)?;
                let stamp = time(val.trim())?;
                ensure!(in_track, "INDEX outside TRACK");
                if ix == "01" {
                    let d = tracks.last_mut().unwrap().disc.as_mut().unwrap();
                    ensure!(d.start == u64::MAX, "Duplicate INDEX 01");
                    d.start = stamp;
                }
            }
            "PREGAP" | "POSTGAP" if time(args)? != 0 => {
                bail!("Synthesized CUE gaps are unsupported")
            }
            _ => {}
        }
    }
    ensure!(!tracks.is_empty(), "Sheet contains no audio tracks");
    for i in (0..tracks.len()).rev() {
        let end = tracks
            .get(i + 1)
            .filter(|next| next.source == tracks[i].source)
            .and_then(|next| next.disc.as_ref().map(|d| d.start));
        let d = tracks[i].disc.as_mut().unwrap();
        ensure!(
            d.start != u64::MAX && end.is_none_or(|end| end > d.start),
            "Missing or unordered INDEX 01"
        );
        d.end = end;
        tracks[i].duration = end.map(|end| (end - d.start) as f64 / 75.);
    }
    // Keep recorded audio before the first INDEX 01 reachable as a separate item.
    let mut result = Vec::new();
    let mut previous = None;
    for t in tracks {
        let d = t.disc.as_ref().unwrap();
        if previous.as_ref() != Some(&t.source) && d.start > 0 {
            let mut pre = t.clone();
            pre.title = "Disc lead-in".into();
            pre.disc = Some(DiscTrack {
                number: 0,
                start: 0,
                end: Some(d.start),
                sheet: sheet.into(),
            });
            pre.duration = Some(d.start as f64 / 75.);
            result.push(pre);
        }
        previous = Some(t.source.clone());
        result.push(t);
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_boundaries_and_recorded_pregaps() {
        let text = "TITLE Set\nFILE a.flac WAVE\nTRACK 01 AUDIO\nINDEX 01 00:01:00\nTRACK 02 AUDIO\nINDEX 00 00:02:00\nINDEX 01 00:02:01\nFILE b.wav WAVE\nTRACK 03 AUDIO\nINDEX 01 00:00:00";
        let p = parse(Path::new("album.cue"), text).unwrap();
        assert_eq!(p.len(), 4);
        assert_eq!(p[0].disc.as_ref().unwrap().end, Some(75));
        assert_eq!(p[1].disc.as_ref().unwrap().end, Some(151));
        assert_eq!(p[2].disc.as_ref().unwrap().end, None);
        assert_eq!(p[3].source, Path::new("b.wav"));
    }
    #[test]
    fn incomplete_sheet_does_not_mask_source() {
        assert!(parse(Path::new("a.cue"), "FILE a.wav WAVE\nTRACK 01 AUDIO").is_err());
    }
}
