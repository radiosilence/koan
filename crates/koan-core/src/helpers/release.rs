//! What kind of release a record is: an album, an EP, a live recording.
//!
//! The types are MusicBrainz's release group types, as Picard writes them to
//! tags (`RELEASETYPE`, `MusicBrainz Album Type`) and OpenSubsonic servers
//! send them (`releaseTypes`): one primary type and any number of secondary
//! ones. They are stored folded, `;`-separated, primary type first where the
//! source gives it first.

/// The types each value names, folded and joined for storage. A tag may hold
/// several values or one value naming several types (`album; live`,
/// `ep/remix`). `None` where nothing is named.
pub fn release_types<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut types: Vec<String> = Vec::new();
    for value in values {
        for name in value.split([';', '/', ',', '\0']) {
            let name = name.trim().to_lowercase();
            if !name.is_empty() && !types.contains(&name) {
                types.push(name);
            }
        }
    }
    (!types.is_empty()).then(|| types.join(";"))
}

/// The secondary types that decide a record's section ahead of any other.
const SPOKEN_WORD: &[&str] = &["spokenword", "interview", "audiobook", "audio drama"];
const MIXES: &[&str] = &["dj-mix", "mixtape", "street"];

/// The heading a record sits under on its artist's page, in page order. An
/// artist's albums are listed in this order (`koan_release_section`), so a
/// client draws a heading wherever the section changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReleaseSection {
    /// Albums, live or not, and every record no other section claims: a
    /// soundtrack, a remix album, a broadcast, a record whose type nobody
    /// tagged. An untagged library reads as one list with no headings.
    Albums,
    SinglesAndEps,
    Mixes,
    SpokenWord,
    Compilations,
}

impl ReleaseSection {
    /// A record carrying several types (`album;compilation;dj-mix`) goes to
    /// the first section that claims one of them: spoken word, then mixes,
    /// compilations, singles and EPs, and albums last. Live and remix claim
    /// nothing, so a live EP is an EP and a live album an album.
    pub fn of(types: Option<&str>) -> Self {
        let types = names(types);
        let has = |wanted: &[&str]| types.iter().any(|t| wanted.contains(t));
        if has(SPOKEN_WORD) {
            Self::SpokenWord
        } else if has(MIXES) {
            Self::Mixes
        } else if has(&["compilation"]) {
            Self::Compilations
        } else if has(&["single", "ep", "demo"]) {
            Self::SinglesAndEps
        } else {
            Self::Albums
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Albums => "Albums / Live Albums",
            Self::SinglesAndEps => "Singles / EPs / Demos",
            Self::Mixes => "Mixes / Mixtapes",
            Self::SpokenWord => "Spokenword / Interview / Audiobook",
            Self::Compilations => "Compilations",
        }
    }
}

/// The few words a record's tile shows for its type: `EP`, `Live`, `Remix
/// EP`. `None` for an album and for a record whose type is not known, which
/// is what a tile without one is taken to be. A spoken word or mix type is
/// named ahead of the others, as it decides the record's section: a mixed
/// compilation reads `DJ Mix` under Mixes.
pub fn release_badge(types: Option<&str>) -> Option<String> {
    let types = names(types);
    let primary = types.iter().find_map(|t| match *t {
        "ep" => Some("EP"),
        "single" => Some("Single"),
        "broadcast" => Some("Broadcast"),
        _ => None,
    });
    let secondary = types
        .iter()
        .find(|t| SPOKEN_WORD.contains(*t) || MIXES.contains(*t))
        .or_else(|| types.iter().find(|t| secondary_label(t).is_some()))
        .and_then(|t| secondary_label(t));
    match (secondary, primary) {
        (Some(s), Some(p)) if p != "Broadcast" => Some(format!("{s} {p}")),
        (Some(s), _) => Some(s.to_string()),
        (None, p) => p.map(str::to_string),
    }
}

fn names(types: Option<&str>) -> Vec<&str> {
    types
        .unwrap_or_default()
        .split(';')
        .filter(|t| !t.is_empty())
        .collect()
}

fn secondary_label(name: &str) -> Option<&'static str> {
    Some(match name {
        "compilation" => "Compilation",
        "live" => "Live",
        "soundtrack" => "Soundtrack",
        "remix" => "Remix",
        "dj-mix" => "DJ Mix",
        // `mixtape/street`, split on its slash as an ID3 list would be.
        "mixtape" | "street" => "Mixtape",
        "demo" => "Demo",
        "spokenword" => "Spoken Word",
        "interview" => "Interview",
        "audiobook" => "Audiobook",
        "audio drama" => "Audio Drama",
        "field recording" => "Field Recording",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_and_splits_tag_values() {
        assert_eq!(
            release_types(["Album", "Live"]).as_deref(),
            Some("album;live")
        );
        assert_eq!(
            release_types(["album; compilation"]).as_deref(),
            Some("album;compilation")
        );
        assert_eq!(
            release_types(["EP/Remix", "ep"]).as_deref(),
            Some("ep;remix")
        );
        assert_eq!(release_types(["", " "]), None);
    }

    #[test]
    fn mixtape_survives_the_slash_split() {
        // `mixtape/street` is one type, but splits on `/` like an ID3 list.
        let types = release_types(["album", "mixtape/street"]);
        assert_eq!(types.as_deref(), Some("album;mixtape;street"));
        assert_eq!(release_badge(types.as_deref()).as_deref(), Some("Mixtape"));
        assert_eq!(ReleaseSection::of(types.as_deref()), ReleaseSection::Mixes);
    }

    #[test]
    fn badges() {
        assert_eq!(release_badge(None), None);
        assert_eq!(release_badge(Some("album")), None);
        assert_eq!(release_badge(Some("ep")).as_deref(), Some("EP"));
        assert_eq!(release_badge(Some("single")).as_deref(), Some("Single"));
        assert_eq!(release_badge(Some("album;live")).as_deref(), Some("Live"));
        assert_eq!(release_badge(Some("ep;remix")).as_deref(), Some("Remix EP"));
        assert_eq!(
            release_badge(Some("broadcast;live")).as_deref(),
            Some("Live")
        );
        assert_eq!(release_badge(Some("other")), None);
        assert_eq!(
            release_badge(Some("album;compilation;dj-mix")).as_deref(),
            Some("DJ Mix")
        );
        assert_eq!(
            release_badge(Some("album;compilation;spokenword")).as_deref(),
            Some("Spoken Word")
        );
        assert_eq!(
            release_badge(Some("album;live;interview")).as_deref(),
            Some("Interview")
        );
        assert_eq!(
            release_badge(Some("ep;remix;dj-mix")).as_deref(),
            Some("DJ Mix EP")
        );
        assert_eq!(
            release_badge(Some("album;live;compilation")).as_deref(),
            Some("Live")
        );
    }

    #[test]
    fn sections() {
        use ReleaseSection::*;
        let cases = [
            (None, Albums),
            (Some("album"), Albums),
            (Some("album;live"), Albums),
            (Some("live"), Albums),
            (Some("broadcast;live"), Albums),
            (Some("album;soundtrack"), Albums),
            (Some("album;remix"), Albums),
            (Some("broadcast"), Albums),
            (Some("other"), Albums),
            (Some("album;field recording"), Albums),
            (Some("something-new"), Albums),
            (Some("ep"), SinglesAndEps),
            (Some("single"), SinglesAndEps),
            (Some("ep;live"), SinglesAndEps),
            (Some("ep;remix"), SinglesAndEps),
            (Some("single;demo"), SinglesAndEps),
            (Some("album;demo"), SinglesAndEps),
            (Some("album;dj-mix"), Mixes),
            (Some("album;mixtape;street"), Mixes),
            (Some("ep;dj-mix"), Mixes),
            (Some("album;compilation;dj-mix"), Mixes),
            (Some("album;spokenword"), SpokenWord),
            (Some("other;interview"), SpokenWord),
            (Some("album;audiobook"), SpokenWord),
            (Some("other;audio drama"), SpokenWord),
            (Some("album;compilation;spokenword"), SpokenWord),
            (Some("album;dj-mix;interview"), SpokenWord),
            (Some("album;compilation"), Compilations),
            (Some("album;compilation;live"), Compilations),
            (Some("single;compilation"), Compilations),
        ];
        for (types, section) in cases {
            assert_eq!(ReleaseSection::of(types), section, "{types:?}");
        }
    }

    #[test]
    fn page_order() {
        use ReleaseSection::*;
        let mut sections = [Compilations, SpokenWord, Mixes, SinglesAndEps, Albums];
        sections.sort();
        assert_eq!(
            sections,
            [Albums, SinglesAndEps, Mixes, SpokenWord, Compilations]
        );
    }
}
