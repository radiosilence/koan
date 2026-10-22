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

/// The heading a record sits under on its artist's page, in page order. An
/// artist's albums are listed in this order (`koan_release_section`), so a
/// client draws a heading wherever the section changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReleaseSection {
    /// Albums, and every record whose type nobody tagged: an untagged library
    /// reads as it always has.
    Albums,
    EpsAndSingles,
    Live,
    Compilations,
    Other,
}

impl ReleaseSection {
    pub fn of(types: Option<&str>) -> Self {
        let types = names(types);
        let has = |t: &str| types.contains(&t);
        if has("live") {
            Self::Live
        } else if has("compilation") {
            Self::Compilations
        } else if has("ep") || has("single") {
            Self::EpsAndSingles
        } else if has("broadcast")
            || has("other")
            || types.iter().any(|t| secondary_label(t).is_some())
        {
            Self::Other
        } else {
            Self::Albums
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Albums => "Albums",
            Self::EpsAndSingles => "EPs & Singles",
            Self::Live => "Live",
            Self::Compilations => "Compilations",
            Self::Other => "Other releases",
        }
    }
}

/// The few words a record's tile shows for its type: `EP`, `Live`, `Remix
/// EP`. `None` for an album and for a record whose type is not known, which
/// is what a tile without one is taken to be.
pub fn release_badge(types: Option<&str>) -> Option<String> {
    let types = names(types);
    let primary = types.iter().find_map(|t| match *t {
        "ep" => Some("EP"),
        "single" => Some("Single"),
        "broadcast" => Some("Broadcast"),
        _ => None,
    });
    let secondary = types.iter().find_map(|t| secondary_label(t));
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
        assert_eq!(ReleaseSection::of(types.as_deref()), ReleaseSection::Other);
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
    }

    #[test]
    fn sections() {
        assert_eq!(ReleaseSection::of(None), ReleaseSection::Albums);
        assert_eq!(ReleaseSection::of(Some("album")), ReleaseSection::Albums);
        assert_eq!(
            ReleaseSection::of(Some("ep")),
            ReleaseSection::EpsAndSingles
        );
        assert_eq!(
            ReleaseSection::of(Some("single")),
            ReleaseSection::EpsAndSingles
        );
        assert_eq!(ReleaseSection::of(Some("ep;live")), ReleaseSection::Live);
        assert_eq!(
            ReleaseSection::of(Some("ep;remix")),
            ReleaseSection::EpsAndSingles
        );
        assert_eq!(
            ReleaseSection::of(Some("single;demo")),
            ReleaseSection::EpsAndSingles
        );
        assert_eq!(
            ReleaseSection::of(Some("album;compilation")),
            ReleaseSection::Compilations
        );
        assert_eq!(
            ReleaseSection::of(Some("album;soundtrack")),
            ReleaseSection::Other
        );
        assert_eq!(ReleaseSection::of(Some("broadcast")), ReleaseSection::Other);
        assert_eq!(
            ReleaseSection::of(Some("something-new")),
            ReleaseSection::Albums
        );
    }
}
