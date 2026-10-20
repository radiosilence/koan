//! A credit as it reads, with each artist it names linked.

use std::collections::HashMap;

use rusqlite::Connection;
use unicode_normalization::UnicodeNormalization;

use crate::db::connection::DbError;
use crate::db::queries::batch::{CreditsOf, Place, credited_artists};

/// One stretch of a credit: an artist's name, linked to them, or the plain
/// text joining two names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreditPart {
    pub text: String,
    pub artist_id: Option<i64>,
}

/// `credit` cut into parts, each credited artist's name linked to that artist
/// and the joining text ("×", ", ", " feat. ") plain.
///
/// The names are looked for in credit order, as whole words, ignoring case,
/// whitespace and Unicode normalisation, so a link reads as the credit
/// spells it. When one is not there, as when MusicBrainz spells an artist
/// differently from the credit, the artists are listed joined by commas
/// instead. With fewer than two credited artists the credit is one part,
/// linked to `filed`, the artist the record or track is filed under.
pub fn credit_parts<S: AsRef<str>>(
    credit: &str,
    filed: Option<i64>,
    artists: &[(i64, S)],
) -> Vec<CreditPart> {
    if artists.len() < 2 {
        return vec![CreditPart {
            text: credit.to_string(),
            artist_id: filed,
        }];
    }
    let credit: String = credit.nfc().collect();
    let folded = Folded::new(&credit);
    let mut parts = Vec::with_capacity(artists.len() * 2 + 1);
    let mut at = 0;
    let mut byte = 0;
    for (id, name) in artists {
        let name = Folded::name(name.as_ref());
        let Some(found) = folded.find(&name, at) else {
            return comma_joined(artists);
        };
        let (start, end) = folded.bytes(found, name.len());
        if start > byte {
            parts.push(CreditPart {
                text: credit[byte..start].to_string(),
                artist_id: None,
            });
        }
        parts.push(CreditPart {
            text: credit[start..end].to_string(),
            artist_id: Some(*id),
        });
        at = found + name.len();
        byte = end;
    }
    if byte < credit.len() {
        parts.push(CreditPart {
            text: credit[byte..].to_string(),
            artist_id: None,
        });
    }
    parts
}

/// A queue row's credit and its record's, each with the artists it credits
/// linked: the two credits a row and the heading above it draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowCredits {
    pub artist: Vec<CreditPart>,
    pub album_artist: Vec<CreditPart>,
}

impl RowCredits {
    /// `own` is the artists the track credits, where it has any of its own,
    /// and `record` its record's. A track naming none of its own but credited
    /// as its record is takes the record's.
    pub fn new<S: AsRef<str>>(
        artist: &str,
        album_artist: &str,
        place: Place,
        own: Option<&[(i64, S)]>,
        record: &[(i64, S)],
    ) -> Self {
        let own = own
            .or((artist == album_artist).then_some(record))
            .unwrap_or_default();
        Self {
            artist: credit_parts(artist, place.artist_id, own),
            album_artist: credit_parts(album_artist, place.album_artist_id, record),
        }
    }

    /// The track's credit reads differently from its record's. Grouped under
    /// the record's heading, which names the record's artists, a row names its
    /// own only then. The credits are compared as drawn, so artists a track
    /// credits beyond its record's are shown even where its artist tag is the
    /// record's.
    pub fn differ(&self) -> bool {
        // `credit_parts` normalises a credit it cuts and not one it leaves
        // whole, so the two sides are compared normalised.
        let text =
            |parts: &[CreditPart]| parts.iter().flat_map(|p| p.text.nfc()).collect::<String>();
        text(&self.artist) != text(&self.album_artist)
    }
}

/// The artists credited on a list of tracks and on their records, read once
/// for the whole list: what `RowCredits` is built from for each row.
#[derive(Debug, Default, Clone)]
pub struct QueueCredits {
    /// Keyed by track.
    pub tracks: HashMap<i64, Vec<(i64, String)>>,
    /// Keyed by album.
    pub albums: HashMap<i64, Vec<(i64, String)>>,
}

impl QueueCredits {
    /// The credits of `places`' tracks and of the records they are on.
    pub fn read(conn: &Connection, places: &HashMap<i64, Place>) -> Result<Self, DbError> {
        let tracks: Vec<i64> = places.keys().copied().collect();
        let mut albums: Vec<i64> = places.values().filter_map(|p| p.album_id).collect();
        albums.sort_unstable();
        albums.dedup();
        Ok(Self {
            tracks: credits_by_id(conn, CreditsOf::Tracks, &tracks)?,
            albums: credits_by_id(conn, CreditsOf::Albums, &albums)?,
        })
    }

    /// The credits of a row filed at `place`, `track` where it is a library
    /// track.
    pub fn row(
        &self,
        track: Option<i64>,
        place: Place,
        artist: &str,
        album_artist: &str,
    ) -> RowCredits {
        let record = place
            .album_id
            .and_then(|id| self.albums.get(&id))
            .map_or(&[][..], Vec::as_slice);
        let own = track.and_then(|id| self.tracks.get(&id)).map(Vec::as_slice);
        RowCredits::new(artist, album_artist, place, own, record)
    }
}

/// A credit folded as names are matched: NFC, lowercased a character at a
/// time, whitespace collapsed, final sigma read as sigma. Each folded
/// character keeps the bytes of the credit it came from. Names are folded the
/// same way: a word-final Σ lowers to σ a character at a time and to ς in a
/// whole string, and a name may be spelt either way.
struct Folded {
    chars: Vec<char>,
    spans: Vec<(usize, usize)>,
}

impl Folded {
    fn new(credit: &str) -> Self {
        let mut chars = Vec::with_capacity(credit.len());
        let mut spans: Vec<(usize, usize)> = Vec::with_capacity(credit.len());
        for (at, c) in credit.char_indices() {
            let span = (at, at + c.len_utf8());
            if c.is_whitespace() {
                if chars.last() == Some(&' ') {
                    spans.last_mut().expect("a space was pushed").1 = span.1;
                } else {
                    chars.push(' ');
                    spans.push(span);
                }
            } else {
                for lower in c.to_lowercase() {
                    chars.push(if lower == 'ς' { 'σ' } else { lower });
                    spans.push(span);
                }
            }
        }
        Self { chars, spans }
    }

    fn name(name: &str) -> Vec<char> {
        let name: String = name.nfc().collect();
        let mut chars = Self::new(&name).chars;
        while chars.last() == Some(&' ') {
            chars.pop();
        }
        let lead = chars.iter().take_while(|c| **c == ' ').count();
        chars.split_off(lead)
    }

    /// Where `name` next occurs from `from` as a whole word: not run into a
    /// letter or digit on either side.
    fn find(&self, name: &[char], from: usize) -> Option<usize> {
        if name.is_empty() || name.len() > self.chars.len() {
            return None;
        }
        let word = |i: usize| self.chars.get(i).is_some_and(|c| c.is_alphanumeric());
        (from..=self.chars.len() - name.len()).find(|&i| {
            self.chars[i..i + name.len()] == *name
                && !(name[0].is_alphanumeric() && i > 0 && word(i - 1))
                && !(name[name.len() - 1].is_alphanumeric() && word(i + name.len()))
        })
    }

    /// The credit's bytes for `len` folded characters from `at`.
    fn bytes(&self, at: usize, len: usize) -> (usize, usize) {
        (self.spans[at].0, self.spans[at + len - 1].1)
    }
}

fn comma_joined<S: AsRef<str>>(artists: &[(i64, S)]) -> Vec<CreditPart> {
    let mut parts = Vec::with_capacity(artists.len() * 2 - 1);
    for (i, (id, name)) in artists.iter().enumerate() {
        if i > 0 {
            parts.push(CreditPart {
                text: ", ".to_string(),
                artist_id: None,
            });
        }
        parts.push(CreditPart {
            text: name.as_ref().to_string(),
            artist_id: Some(*id),
        });
    }
    parts
}

/// The credited artists of these tracks or albums, as `credit_parts` takes
/// them: id and name in credit order, keyed by track or album.
pub fn credits_by_id(
    conn: &Connection,
    of: CreditsOf,
    ids: &[i64],
) -> Result<HashMap<i64, Vec<(i64, String)>>, DbError> {
    Ok(credited_artists(conn, of, ids)?
        .into_iter()
        .map(|(owner, artists)| {
            let named = artists
                .into_iter()
                .map(|c| (c.artist.id, c.artist.name))
                .collect();
            (owner, named)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(parts: &[CreditPart]) -> Vec<(&str, Option<i64>)> {
        parts
            .iter()
            .map(|p| (p.text.as_str(), p.artist_id))
            .collect()
    }

    #[test]
    fn each_name_in_the_credit_links_its_artist() {
        let credit = "Orchestre Tout Puissant Marcel Duchamp × Frànçois & the Atlas Mountains";
        let parts = credit_parts(
            credit,
            Some(9),
            &[
                (1, "Orchestre Tout Puissant Marcel Duchamp"),
                (2, "Frànçois & the Atlas Mountains"),
            ],
        );
        assert_eq!(
            read(&parts),
            [
                ("Orchestre Tout Puissant Marcel Duchamp", Some(1)),
                (" × ", None),
                ("Frànçois & the Atlas Mountains", Some(2)),
            ]
        );
    }

    #[test]
    fn text_around_the_names_is_kept_plain() {
        let parts = credit_parts("A feat. B (live)", None, &[(1, "A"), (2, "B")]);
        assert_eq!(
            read(&parts),
            [
                ("A", Some(1)),
                (" feat. ", None),
                ("B", Some(2)),
                (" (live)", None)
            ]
        );
    }

    #[test]
    fn a_name_not_in_the_credit_lists_the_artists_by_commas() {
        let parts = credit_parts("Bjork & Friend", None, &[(1, "Björk"), (2, "Friend")]);
        assert_eq!(
            read(&parts),
            [("Björk", Some(1)), (", ", None), ("Friend", Some(2))]
        );
    }

    #[test]
    fn a_name_is_found_whatever_its_case_and_reads_as_the_credit_spells_it() {
        let parts = credit_parts(
            "Boards Of Canada  &  Hood",
            None,
            &[(1, "Boards of Canada"), (2, "Hood")],
        );
        assert_eq!(
            read(&parts),
            [
                ("Boards Of Canada", Some(1)),
                ("  &  ", None),
                ("Hood", Some(2))
            ]
        );
    }

    #[test]
    fn a_name_is_matched_only_as_a_whole_word() {
        let parts = credit_parts("Sonsie & Sons", None, &[(1, "Sons"), (2, "Sonsie")]);
        assert_eq!(
            read(&parts),
            [("Sons", Some(1)), (", ", None), ("Sonsie", Some(2))]
        );
        let parts = credit_parts("Ann & Annabel", None, &[(1, "Ann"), (2, "Annabel")]);
        assert_eq!(
            read(&parts),
            [("Ann", Some(1)), (" & ", None), ("Annabel", Some(2))]
        );
    }

    #[test]
    fn a_name_ending_in_sigma_is_found() {
        let parts = credit_parts("ΧΆΟΣ & Βήμα", None, &[(1, "Χάος"), (2, "Βήμα")]);
        assert_eq!(
            read(&parts),
            [("ΧΆΟΣ", Some(1)), (" & ", None), ("Βήμα", Some(2))]
        );
    }

    #[test]
    fn names_are_found_in_credit_order() {
        let parts = credit_parts("B & A", None, &[(1, "A"), (2, "B")]);
        assert_eq!(read(&parts), [("A", Some(1)), (", ", None), ("B", Some(2))]);
    }

    #[test]
    fn a_credit_naming_no_artists_links_where_it_is_filed() {
        assert_eq!(
            read(&credit_parts::<&str>("Solo", Some(4), &[])),
            [("Solo", Some(4))]
        );
        assert_eq!(read(&credit_parts::<&str>("", None, &[])), [("", None)]);
    }

    #[test]
    fn a_row_differs_from_its_record_by_its_credit_as_drawn() {
        let place = Place {
            album_id: Some(5),
            artist_id: Some(1),
            album_artist_id: Some(1),
        };
        let record: &[(i64, &str)] = &[];
        let guests: &[(i64, &str)] = &[(1, "Big Data"), (2, "Kimbra")];
        // The tag names only the record's artist; the credit names a guest.
        let row = RowCredits::new("Big Data", "Big Data", place, Some(guests), record);
        assert!(row.differ());
        let row = RowCredits::new("Big Data", "Big Data", place, None, record);
        assert!(!row.differ());
        let row = RowCredits::new("Big Data feat. Kimbra", "Big Data", place, None, record);
        assert!(row.differ());
        // Credited as its record is, a track takes the record's artists.
        let duo: &[(i64, &str)] = &[(1, "A"), (2, "B")];
        let row = RowCredits::new("A × B", "A × B", place, None, duo);
        assert_eq!(row.artist, row.album_artist);
        assert!(!row.differ());
        // A decomposed tag cut into parts on one side and left whole on the
        // other reads the same.
        let solo: &[(i64, &str)] = &[(1, "Ro\u{301}s")];
        let both: &[(i64, &str)] = &[(1, "Ro\u{301}s"), (2, "Kimbra")];
        let row = RowCredits::new(
            "Ro\u{301}s Kimbra",
            "Ro\u{301}s Kimbra",
            place,
            Some(both),
            solo,
        );
        assert!(!row.differ());
    }
}
