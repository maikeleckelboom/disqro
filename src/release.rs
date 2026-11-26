//! Releases: the discs that together make up one authored album.
//!
//! A release is only recognised on evidence that can be pointed to:
//!
//! - **Disc folders.** A folder whose music lives entirely in two or more
//!   subfolders named as discs (`CD1`, `CD 2`, `Disc 3`, `CD1 - Subtitle`),
//!   one disc in each, numbered 1 to n without gaps.
//! - **Disc tags.** Two or more tagged discs in one folder that share an
//!   album and are numbered 1 to n by their disc tags.
//!
//! In both cases the discs' album titles must agree once their disc markers
//! are removed, and a marker in a title must agree with the disc's number.
//! Volumes, editions and compilations in neighbouring folders never merge, and
//! anything ambiguous stays a set of independent discs: a missed release is
//! better than a wrong one.
use std::{collections::HashMap, hash::Hash, path::Path};

/// The Releases order projected from an already ordered collection.
/// A release occupies the position of its first authored disc. Its other
/// discs are moved beneath it in authored order, regardless of their sort keys.
pub enum Ordered {
    Disc(usize),
    Release(Vec<usize>),
}

pub fn ordered<K: Eq + Hash>(
    membership: impl IntoIterator<Item = Option<(K, u32)>>,
) -> Vec<Ordered> {
    let mut groups = HashMap::<K, Vec<(u32, usize)>>::new();
    let mut order = Vec::new();
    for (index, member) in membership.into_iter().enumerate() {
        if let Some((key, number)) = member {
            groups.entry(key).or_default().push((number, index));
            order.push(None);
        } else {
            order.push(Some(Ordered::Disc(index)));
        }
    }
    for mut group in groups.into_values() {
        group.sort_unstable();
        let first = group[0].1;
        order[first] = Some(Ordered::Release(
            group.into_iter().map(|(_, index)| index).collect(),
        ));
    }
    order.into_iter().flatten().collect()
}

/// What release detection needs to know about one disc.
pub struct Candidate<'a> {
    /// The folder holding the disc's sheet or files.
    pub folder: &'a Path,
    /// The album title as authored, or a name derived from a file.
    pub title: &'a str,
    /// A tagged disc number, or one read from the disc's folder.
    pub number: Option<u32>,
}

/// Discs that belong together, in disc order.
#[derive(Clone, Debug, PartialEq)]
pub struct Grouping {
    /// Indices into the candidates, CD 1 first.
    pub discs: Vec<usize>,
    pub title: String,
    /// What distinguishes each disc beyond its number, such as the
    /// `Filthy Kicks` of `CD2 - Filthy Kicks`; often empty.
    pub subtitles: Vec<String>,
}

/// A disc marker such as `CD1`, `CD 2` or `Disc 03`, found at a word
/// boundary: its byte range and number.
fn marker(s: &str) -> Option<(std::ops::Range<usize>, u32)> {
    let lower = s.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut from = 0;
    while from < bytes.len() {
        let (at, word) = ["cd", "disc", "disk"]
            .iter()
            .filter_map(|w| lower[from..].find(w).map(|i| (from + i, *w)))
            .min_by_key(|(i, w)| (*i, std::cmp::Reverse(w.len())))?;
        from = at + 1;
        if at > 0 && bytes[at - 1].is_ascii_alphanumeric() {
            continue;
        }
        let mut i = at + word.len();
        while i < bytes.len() && matches!(bytes[i], b' ' | b'_' | b'-' | b'.' | b'#') {
            i += 1;
        }
        let digits = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == digits || (i < bytes.len() && bytes[i].is_ascii_alphanumeric()) {
            continue;
        }
        if let Ok(number) = lower[digits..i].parse::<u32>()
            && number > 0
        {
            return Some((at..i, number));
        }
    }
    None
}

/// The number and subtitle of a folder named as a disc, such as `CD1`,
/// `Disc 2` or `CD1 - Electronic Overdrive`.
pub fn disc_folder(name: &str) -> Option<(u32, &str)> {
    let name = name.trim();
    let (range, number) = marker(name).filter(|(r, _)| r.start == 0)?;
    Some((number, trim_separators(&name[range.end..])))
}

pub(crate) fn names_cd(name: &str) -> bool {
    marker(name)
        .is_some_and(|(range, _)| name[range.start..range.start + 2].eq_ignore_ascii_case("cd"))
}

fn trim_separators(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || "-–—_,.:;".contains(c))
}

/// Letters and digits only, so punctuation and spacing differences between
/// the sheets of one release do not matter.
fn normalized(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A disc title without its disc marker: the shared part before the marker,
/// and what follows it. A marker that encloses a whole parenthetical, as in
/// `Title (CD1)`, leaves no empty brackets behind.
struct Split<'a> {
    base: String,
    rest: &'a str,
    number: Option<u32>,
}
fn split(title: &str) -> Split<'_> {
    let Some((range, number)) = marker(title) else {
        return Split {
            base: title.trim().to_owned(),
            rest: "",
            number: None,
        };
    };
    let mut base = title[..range.start].trim_end().to_owned();
    let mut rest = &title[range.end..];
    let closing = rest.trim_start().strip_prefix([')', ']']);
    if base.ends_with(['(', '[']) {
        base.pop();
        if let Some(after) = closing {
            rest = after;
        }
    } else if let Some(after) = closing
        && base.matches(['(', '[']).count() > base.matches([')', ']']).count()
    {
        base.push(')');
        rest = after;
    }
    Split {
        base: trim_separators(&base).to_owned(),
        rest: trim_separators(rest),
        number: Some(number),
    }
}

/// The release title a disc implies, given the subtitle its folder names.
fn base_title(title: &str, number: u32, subtitle: &str) -> Option<String> {
    let s = split(title);
    if s.number.is_some_and(|n| n != number) {
        return None;
    }
    if s.number.is_none() && !subtitle.is_empty() {
        let (whole, sub) = (normalized(title), normalized(subtitle));
        if let Some(prefix) = whole.strip_suffix(&sub)
            && !prefix.is_empty()
        {
            // Cut the same number of letters and digits from the original.
            let mut keep = prefix.chars().count();
            let end = title
                .char_indices()
                .find(|(_, c)| {
                    if c.is_alphanumeric() {
                        if keep == 0 {
                            return true;
                        }
                        keep -= 1;
                    }
                    false
                })
                .map_or(title.len(), |(i, _)| i);
            return Some(trim_separators(&title[..end]).to_owned());
        }
    }
    Some(s.base)
}

/// Explicit edition labels following a disc marker can sit beside ordinary
/// subtitles, as in `(2020 Remaster) - Early Rave`.
fn edition_labels(suffix: &str) -> impl Iterator<Item = String> {
    suffix
        .split(['(', ')', '[', ']', '–', '—'])
        .flat_map(|part| part.split(" - "))
        .map(str::trim)
        .filter(|label| {
            label.eq_ignore_ascii_case("original")
                || label.split(|c: char| !c.is_alphanumeric()).any(|word| {
                    ["edition", "remaster", "remastered"]
                        .iter()
                        .any(|term| word.eq_ignore_ascii_case(term))
                })
        })
        .map(normalized)
}

/// Discs must agree on explicit edition labels. Other suffixes may name
/// individual discs, or form part of the shared title, such as `(Light Grey)`.
fn group_title(titles: &[&str]) -> Option<String> {
    let splits: Vec<_> = titles.iter().map(|t| split(t)).collect();
    let first = &splits[0];
    if splits[1..]
        .iter()
        .any(|s| edition_labels(s.rest).ne(edition_labels(first.rest)))
    {
        return None;
    }
    let suffix = normalized(first.rest);
    let shared = !suffix.is_empty() && splits.iter().all(|s| normalized(s.rest) == suffix);
    Some(if shared {
        format!("{} {}", first.base, first.rest)
    } else {
        first.base.clone()
    })
}

/// What a disc's title or folder adds to the release title.
fn subtitle(title: &str, folder: &str, release: &str) -> String {
    let s = split(title);
    let extra = if s.number.is_some() || folder.is_empty() {
        s.rest
    } else {
        folder
    };
    if normalized(release).ends_with(&normalized(extra)) {
        String::new()
    } else {
        extra.to_owned()
    }
}

/// A path as Windows compares it.
fn key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// Groups candidates into releases. Candidates outside every group are
/// independent discs.
///
/// Discovery groups again as each batch arrives, so this allocates one key
/// per disc and reads titles only for discs that could form a release.
pub fn releases(candidates: &[Candidate]) -> Vec<Grouping> {
    let keys: Vec<String> = candidates.iter().map(|c| key(c.folder)).collect();
    let mut sorted: Vec<&str> = keys.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    // How many discs a folder holds: itself, or itself and beneath it.
    let range = |from: &str, to: &str| {
        sorted.partition_point(|k| *k < to) - sorted.partition_point(|k| *k < from)
    };
    let within = |folder: &str| range(folder, &format!("{folder}\0"));
    let beneath =
        |folder: &str| within(folder) + range(&format!("{folder}\\"), &format!("{folder}]"));
    let mut out = vec![];
    let mut claimed = vec![false; candidates.len()];

    // Each release folder's disc folders: candidate, number and subtitle.
    let mut by_parent: HashMap<String, Vec<(usize, u32, &str)>> = HashMap::new();
    for (i, c) in candidates.iter().enumerate() {
        let Some(name) = c.folder.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let (Some((number, subtitle)), Some(parent)) = (disc_folder(name), c.folder.parent())
        else {
            continue;
        };
        by_parent
            .entry(key(parent))
            .or_default()
            .push((i, number, subtitle));
    }
    let mut parents: Vec<_> = by_parent.into_iter().collect();
    parents.sort_by(|a, b| a.0.cmp(&b.0));
    for (parent, mut discs) in parents {
        discs.sort_by_key(|(_, n, _)| *n);
        let numbered = discs.len() >= 2
            && discs
                .iter()
                .enumerate()
                .all(|(i, (_, n, _))| *n as usize == i + 1);
        // Nothing else plays inside the release folder, and each disc
        // folder holds exactly one disc whose tags, if any, agree.
        let exclusive = beneath(&parent) == discs.len()
            && discs.iter().all(|(i, n, _)| {
                within(&keys[*i]) == 1 && candidates[*i].number.is_none_or(|t| t == *n)
            });
        if !numbered || !exclusive {
            continue;
        }
        let bases: Option<Vec<String>> = discs
            .iter()
            .map(|(i, n, sub)| base_title(candidates[*i].title, *n, sub))
            .collect();
        let Some(bases) = bases else { continue };
        let reference = normalized(&bases[0]);
        if !bases.iter().all(|b| normalized(b) == reference) {
            continue;
        }
        let titles: Vec<&str> = discs.iter().map(|(i, ..)| candidates[*i].title).collect();
        let Some(shared_title) = group_title(&titles) else {
            continue;
        };
        let mut title = if reference.is_empty() {
            String::new()
        } else if split(titles[0]).number.is_some() {
            shared_title
        } else {
            bases[0].clone()
        };
        if normalized(&title).is_empty() {
            title = candidates[discs[0].0]
                .folder
                .parent()
                .map(crate::library::folder_name)
                .unwrap_or_default();
        }
        for (i, ..) in &discs {
            claimed[*i] = true;
        }
        out.push(Grouping {
            subtitles: discs
                .iter()
                .map(|(i, _, sub)| subtitle(candidates[*i].title, sub, &title))
                .collect(),
            discs: discs.into_iter().map(|(i, ..)| i).collect(),
            title,
        });
    }

    // Disc tags within one folder: only folders holding a tagged disc
    // number are read, but every disc in them takes part.
    let tagged: std::collections::HashSet<&str> = candidates
        .iter()
        .enumerate()
        .filter(|(i, c)| !claimed[*i] && c.number.is_some())
        .map(|(i, _)| keys[i].as_str())
        .collect();
    let mut by_album: HashMap<(&str, String), Vec<usize>> = HashMap::new();
    for (i, c) in candidates.iter().enumerate() {
        if claimed[i] || !tagged.contains(keys[i].as_str()) {
            continue;
        }
        let base = normalized(&split(c.title).base);
        if !base.is_empty() {
            by_album.entry((&keys[i], base)).or_default().push(i);
        }
    }
    let mut albums: Vec<_> = by_album.into_iter().collect();
    albums.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, mut discs) in albums {
        if discs.len() < 2 || discs.iter().any(|&i| candidates[i].number.is_none()) {
            continue;
        }
        discs.sort_by_key(|&i| candidates[i].number);
        let consistent = discs.iter().enumerate().all(|(k, &i)| {
            let c = &candidates[i];
            c.number == Some(k as u32 + 1)
                && split(c.title).number.is_none_or(|n| Some(n) == c.number)
        });
        if !consistent {
            continue;
        }
        let titles: Vec<&str> = discs.iter().map(|&i| candidates[i].title).collect();
        let Some(title) = group_title(&titles) else {
            continue;
        };
        out.push(Grouping {
            subtitles: titles.iter().map(|t| subtitle(t, "", &title)).collect(),
            title,
            discs,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn group(discs: &[(&str, &str, Option<u32>)]) -> Vec<(Vec<usize>, String)> {
        let folders: Vec<PathBuf> = discs.iter().map(|(f, ..)| PathBuf::from(f)).collect();
        let candidates: Vec<Candidate> = discs
            .iter()
            .zip(&folders)
            .map(|((_, title, number), folder)| Candidate {
                folder,
                title,
                number: *number,
            })
            .collect();
        releases(&candidates)
            .into_iter()
            .map(|g| (g.discs, g.title))
            .collect()
    }

    #[test]
    fn disc_markers_and_folders() {
        assert_eq!(disc_folder("CD1"), Some((1, "")));
        assert_eq!(disc_folder("cd 2"), Some((2, "")));
        assert_eq!(disc_folder("Disc 03"), Some((3, "")));
        assert_eq!(
            disc_folder("CD1 - Electronic Overdrive"),
            Some((1, "Electronic Overdrive"))
        );
        for name in [
            "CD",
            "Covers",
            "CDJ 2",
            "Disco 2",
            "CD1A",
            "Bonus CD1",
            "CD0",
        ] {
            assert_eq!(disc_folder(name), None, "{name}");
        }
        let s = split("Thunderdome 2001 (CD1) (Light Grey)");
        assert_eq!(
            (s.base.as_str(), s.rest, s.number),
            ("Thunderdome 2001", "(Light Grey)", Some(1))
        );
        let s = split("Thunderdome II  - Back From Hell! (Remastered CD2)");
        assert_eq!(s.base, "Thunderdome II  - Back From Hell! (Remastered)");
        assert_eq!(
            split("Various - Thunderdome 2003 Green ( CD2)").base,
            "Various - Thunderdome 2003 Green"
        );
        assert_eq!(
            split("Thunderdome '96 - Dance or Die! - (CD2)").base,
            "Thunderdome '96 - Dance or Die!"
        );
    }

    #[test]
    fn disc_folders_of_one_release_group_in_order() {
        let r = r"M:\TD\[1] Thunderdome I";
        let found = group(&[
            (
                &format!(r"{r}\CD2"),
                "Thunderdome I - F ck Mellow, This Is Hardcore From Hell (CD2)",
                Some(2),
            ),
            (
                &format!(r"{r}\CD1"),
                "Thunderdome I - F*ck Mellow, This Is Hardcore From Hell (CD1)",
                Some(1),
            ),
        ]);
        assert_eq!(
            found,
            [(
                vec![1, 0],
                "Thunderdome I - F*ck Mellow, This Is Hardcore From Hell".into()
            )]
        );
        // Per-disc subtitles in titles or in the disc folders' names.
        let found = group(&[
            (
                r"M:\2008\CD1",
                "Thunderdome 2008 (CD1) - Early Rave - Drokz",
                None,
            ),
            (
                r"M:\2008\CD2",
                "Thunderdome 2008 (CD2) - Hardcore - Evil Activities",
                None,
            ),
            (
                r"M:\2008\CD3",
                "Thunderdome 2008 (CD3) - Industrial - Enzyme",
                None,
            ),
            (
                r"M:\2007\CD1 - Electronic Overdrive",
                "Thunderdome 2007 - Electronic Overdrive",
                None,
            ),
            (
                r"M:\2007\CD2 - Filthy Kicks",
                "Thunderdome 2007 - Filthy Kicks",
                None,
            ),
            (r"M:\Grey\CD1", "Thunderdome 2001 (CD1) (Light Grey)", None),
            (r"M:\Grey\CD2", "Thunderdome 2001 (CD2) (Light Grey)", None),
        ]);
        let titles: Vec<_> = found.iter().map(|(d, t)| (d.len(), t.as_str())).collect();
        assert_eq!(
            titles,
            [
                (2, "Thunderdome 2007"),
                (3, "Thunderdome 2008"),
                (2, "Thunderdome 2001 (Light Grey)")
            ]
        );
    }

    #[test]
    fn ambiguous_discs_stay_independent() {
        let none = |discs: &[(&str, &str, Option<u32>)]| assert_eq!(group(discs), [], "{discs:?}");
        // Neighbouring volumes and editions.
        none(&[
            (r"M:\TD\[1] Thunderdome I", "Thunderdome I", None),
            (r"M:\TD\[2] Thunderdome II", "Thunderdome II", None),
        ]);
        // A gap, a single disc, or a second program in one disc folder.
        none(&[
            (r"M:\A\CD1", "A (CD1)", None),
            (r"M:\A\CD3", "A (CD3)", None),
        ]);
        none(&[(r"M:\A\CD1", "A (CD1)", None)]);
        none(&[
            (r"M:\A\CD1", "A (CD1)", None),
            (r"M:\A\CD1", "A (CD1) alternate", None),
            (r"M:\A\CD2", "A (CD2)", None),
        ]);
        // Other music inside the release folder.
        none(&[
            (r"M:\A\CD1", "A (CD1)", None),
            (r"M:\A\CD2", "A (CD2)", None),
            (r"M:\A\Bonus", "A bonus", None),
        ]);
        none(&[
            (r"M:\A\CD1", "A (CD1)", None),
            (r"M:\A\CD2", "A (CD2)", None),
            (r"M:\A", "Loose", None),
        ]);
        // Two unrelated compilations that happen to sit in CD folders.
        none(&[
            (r"M:\Various\CD1", "Club Hits 1995", None),
            (r"M:\Various\CD2", "Summer Anthems", None),
        ]);
        // A title or tag that contradicts its folder.
        none(&[
            (r"M:\A\CD1", "A (CD2)", None),
            (r"M:\A\CD2", "A (CD1)", None),
        ]);
        none(&[(r"M:\A\CD1", "A", Some(2)), (r"M:\A\CD2", "A", Some(1))]);
        // Untitled discs beside titled ones.
        none(&[(r"M:\A\CD1", "", None), (r"M:\A\CD2", "A (CD2)", None)]);
    }

    #[test]
    fn edition_qualifiers_after_disc_markers() {
        for [one, two] in [[r"M:\Atlas\CD1", r"M:\Atlas\CD2"], [r"M:\Atlas"; 2]] {
            let pair = |a, b| group(&[(one, a, Some(1)), (two, b, Some(2))]);
            let original = "Atlas (CD1) (Original Edition)";
            assert!(pair(original, "Atlas (CD2) (Remastered Edition)").is_empty());
            assert!(pair(original, "Atlas (CD2)").is_empty());
            assert_eq!(
                pair(original, "Atlas (CD2) [ORIGINAL EDITION]"),
                [(vec![0, 1], "Atlas (Original Edition)".into())]
            );
            assert_eq!(
                pair("Atlas (CD1) - Original Mix", "Atlas (CD2) - Live Set"),
                [(vec![0, 1], "Atlas".into())]
            );
            assert_eq!(
                pair(
                    "Atlas (CD1) (2020 Remaster) - Early Rave",
                    "Atlas (CD2) (2020 Remaster) - Hardcore",
                ),
                [(vec![0, 1], "Atlas".into())]
            );
        }
    }

    #[test]
    fn disc_tags_in_one_folder() {
        let found = group(&[
            (r"M:\Album", "Live (Disc 2)", Some(2)),
            (r"M:\Album", "Live (Disc 1)", Some(1)),
            (r"M:\Album", "Other", None),
        ]);
        assert_eq!(found, [(vec![1, 0], "Live".into())]);
        // Untagged or partly tagged discs, and different albums, stay apart.
        assert_eq!(
            group(&[(r"M:\Album", "Live", Some(1)), (r"M:\Album", "Live", None)]),
            []
        );
        assert_eq!(
            group(&[
                (r"M:\Album", "Live", Some(1)),
                (r"M:\Album", "Studio", Some(2))
            ]),
            []
        );
        assert_eq!(
            group(&[(r"M:\A", "Live", Some(1)), (r"M:\B", "Live", Some(2))]),
            []
        );
        // Untitled disc folders form a release on structure alone.
        let found = group(&[(r"M:\Rip\CD1", "CD1", None), (r"M:\Rip\CD2", "CD2", None)]);
        assert_eq!(found, [(vec![0, 1], "Rip".into())]);
    }
}
