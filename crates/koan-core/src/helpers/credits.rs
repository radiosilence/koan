//! A credit as it reads, with each artist it names linked.

use std::collections::HashMap;

use rusqlite::Connection;
use unicode_normalization::UnicodeNormalization;

use crate::db::connection::DbError;
use crate::db::queries::batch::{CreditsOf, credited_artists};
use crate::db::queries::sources::fold;

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
/// whitespace and Unicode normalisation (`fold`), so a link reads as the credit
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
        let name: Vec<char> = fold(name.as_ref()).chars().collect();
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

/// A credit as `fold` sees it, each folded character keeping the bytes of the
/// credit it came from.
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
                    chars.push(lower);
                    spans.push(span);
                }
            }
        }
        Self { chars, spans }
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
}
