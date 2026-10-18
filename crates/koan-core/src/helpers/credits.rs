//! A credit as it reads, with each artist it names linked.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::db::connection::DbError;
use crate::db::queries::batch::{CreditsOf, credited_artists};

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
/// The names are looked for in credit order, verbatim. When one is not there,
/// as when MusicBrainz spells an artist differently from the credit, the
/// artists are listed joined by commas instead. With fewer than two credited
/// artists the credit is one part, linked to `filed`, the artist the record
/// or track is filed under.
pub fn credit_parts(credit: &str, filed: Option<i64>, artists: &[(i64, &str)]) -> Vec<CreditPart> {
    if artists.len() < 2 {
        return vec![CreditPart {
            text: credit.to_string(),
            artist_id: filed,
        }];
    }
    let plain = |text: &str| CreditPart {
        text: text.to_string(),
        artist_id: None,
    };
    let mut parts = Vec::with_capacity(artists.len() * 2 + 1);
    let mut rest = credit;
    for &(id, name) in artists {
        let Some(at) = rest.find(name).filter(|_| !name.is_empty()) else {
            return comma_joined(artists);
        };
        if at > 0 {
            parts.push(plain(&rest[..at]));
        }
        parts.push(CreditPart {
            text: name.to_string(),
            artist_id: Some(id),
        });
        rest = &rest[at + name.len()..];
    }
    if !rest.is_empty() {
        parts.push(plain(rest));
    }
    parts
}

fn comma_joined(artists: &[(i64, &str)]) -> Vec<CreditPart> {
    let mut parts = Vec::with_capacity(artists.len() * 2 - 1);
    for (i, &(id, name)) in artists.iter().enumerate() {
        if i > 0 {
            parts.push(CreditPart {
                text: ", ".to_string(),
                artist_id: None,
            });
        }
        parts.push(CreditPart {
            text: name.to_string(),
            artist_id: Some(id),
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

/// `credit_parts` over what `credits_by_id` read for one track or album.
pub fn parts_of(
    credit: &str,
    filed: Option<i64>,
    artists: Option<&Vec<(i64, String)>>,
) -> Vec<CreditPart> {
    let named: Vec<(i64, &str)> = artists
        .map(|a| a.iter().map(|(id, name)| (*id, name.as_str())).collect())
        .unwrap_or_default();
    credit_parts(credit, filed, &named)
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
    fn names_are_found_in_credit_order() {
        let parts = credit_parts("B & A", None, &[(1, "A"), (2, "B")]);
        assert_eq!(read(&parts), [("A", Some(1)), (", ", None), ("B", Some(2))]);
    }

    #[test]
    fn a_credit_naming_no_artists_links_where_it_is_filed() {
        assert_eq!(
            read(&credit_parts("Solo", Some(4), &[])),
            [("Solo", Some(4))]
        );
        assert_eq!(read(&credit_parts("", None, &[])), [("", None)]);
    }
}
