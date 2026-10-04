//! Sorting and filtering the album and artist browsers.
//!
//! The state is the page's query string and nothing else: a reload, the back
//! button and a copied link all land on the same listing, and the server reads
//! it straight into the SQL that narrows, orders and pages the library.

use std::fmt::Write as _;

use koan_core::db::queries::{AlbumFilter, AlbumOrder, AlbumQuery, ArtistOrder, ArtistQuery};
use serde::Deserialize;

use crate::share::escape;

/// A browser's query string. Blank values, as an unfilled form field sends
/// them, mean unset.
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(super) struct Browse {
    sort: String,
    /// Fixes a random order, so paging through it stays one shuffle.
    seed: Option<i64>,
    /// The name filter: album title or artist name on the album browser, the
    /// artist's name on the artist browser.
    q: String,
    fav: String,
    lossless: String,
    codec: String,
    from: String,
    to: String,
    genre: String,
    pub(super) offset: u32,
}

/// Album sorts, as the macOS app offers them.
const ALBUM_SORTS: [(&str, &str); 5] = [
    ("recent", "Recently added"),
    ("title", "Title"),
    ("artist", "Artist"),
    ("year", "Year"),
    ("random", "Random"),
];

const ARTIST_SORTS: [(&str, &str); 3] = [
    ("name", "Name"),
    ("albums", "Most albums"),
    ("recent", "Recently added"),
];

fn set(s: &str) -> Option<&str> {
    Some(s.trim()).filter(|s| !s.is_empty())
}

impl Browse {
    /// A seed for a random order that arrived without one.
    pub(super) fn seeded(mut self) -> Self {
        if self.sort == "random" && self.seed.is_none() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos() as i64 ^ d.as_secs() as i64);
            self.seed = Some(now);
        }
        self
    }

    fn filter(&self) -> AlbumFilter<'_> {
        let year = |s: &str| set(s).and_then(|y| y.parse().ok());
        AlbumFilter {
            lossless: set(&self.lossless).is_some(),
            codec: set(&self.codec),
            year_from: year(&self.from),
            year_to: year(&self.to),
            genre: set(&self.genre),
        }
    }

    /// `user` is whose favourites the favourites toggle narrows to.
    pub(super) fn albums(&self, user: i64, limit: u32) -> AlbumQuery<'_> {
        let order = match self.sort.as_str() {
            "title" => AlbumOrder::Title,
            "artist" => AlbumOrder::ArtistThenDate,
            "year" => AlbumOrder::YearDesc,
            "random" => AlbumOrder::Random(self.seed.unwrap_or(0)),
            _ => AlbumOrder::RecentlyAdded,
        };
        AlbumQuery {
            order,
            search: set(&self.q),
            favourites_of: set(&self.fav).map(|_| user),
            filter: self.filter(),
            limit: Some(limit),
            offset: self.offset,
            ..Default::default()
        }
    }

    /// `user` is whose favourites the favourites toggle narrows to.
    pub(super) fn artists(&self, user: i64, limit: u32) -> ArtistQuery<'_> {
        let order = match self.sort.as_str() {
            "albums" => ArtistOrder::AlbumCount,
            "recent" => ArtistOrder::RecentlyAdded,
            _ => ArtistOrder::Name,
        };
        ArtistQuery {
            order,
            search: set(&self.q),
            favourites_of: set(&self.fav).map(|_| user),
            filter: self.filter(),
            limit: Some(limit),
            offset: self.offset,
            ..Default::default()
        }
    }

    /// How many filters are on, for the collapsed toolbar's label.
    fn active(&self) -> usize {
        [&self.q, &self.fav, &self.lossless, &self.codec, &self.genre]
            .into_iter()
            .filter(|v| set(v).is_some())
            .count()
            + usize::from(set(&self.from).is_some() || set(&self.to).is_some())
    }

    /// The query string for this state at `offset`, blank values dropped.
    /// Percent-encoded throughout, so it is safe inside an attribute and a
    /// quoted script string alike.
    pub(super) fn query(&self, offset: u32) -> String {
        let mut q = form_urlencoded::Serializer::new(String::new());
        let seed = self.seed.map(|s| s.to_string()).unwrap_or_default();
        let offset = if offset > 0 {
            offset.to_string()
        } else {
            String::new()
        };
        for (k, v) in [
            ("sort", self.sort.as_str()),
            ("seed", &seed),
            ("q", &self.q),
            ("fav", &self.fav),
            ("lossless", &self.lossless),
            ("codec", &self.codec),
            ("from", &self.from),
            ("to", &self.to),
            ("genre", &self.genre),
            ("offset", &offset),
        ] {
            if let Some(v) = set(v) {
                q.append_pair(k, v);
            }
        }
        q.finish()
    }
}

/// A labelled control in the toolbar; on a phone, label and control at either
/// end of a row of the sheet.
const LABEL: &str = "inline-flex items-center gap-1.5 max-wide:justify-between";
/// The toolbar's selects and year fields, smaller than a form's.
const FIELD: &str = "bg-surface px-2 py-[5px] text-[13px] max-wide:text-[16px]";

fn select(name: &str, label: &str, options: &[(String, String)], current: &str) -> String {
    let mut out = format!(
        "<label class=\"{LABEL}\">{label}<select class=\"max-w-[12em] {FIELD}\" name={name}>"
    );
    for (value, text) in options {
        let _ = write!(
            out,
            "<option value=\"{v}\"{sel}>{t}</option>",
            v = escape(value),
            t = escape(text),
            sel = if value == current { " selected" } else { "" },
        );
    }
    out.push_str("</select></label>");
    out
}

fn check(name: &str, label: &str, on: &str) -> String {
    format!(
        "<label class=\"inline-flex items-center gap-1.5\"><input class=\"accent-brand\" type=checkbox name={name} \
value=1{}>{label}</label>",
        if set(on).is_some() { " checked" } else { "" }
    )
}

/// The sort and filter controls: a row above the grid on a wide screen, one
/// "Sort · Filter" button that opens them on a phone. A plain GET form, so it
/// works without script; with it, a change applies at once.
pub(super) fn toolbar(
    b: &Browse,
    path: &str,
    artists: bool,
    codecs: &[String],
    genres: &[String],
) -> String {
    let sorts: Vec<(String, String)> = if artists {
        &ARTIST_SORTS[..]
    } else {
        &ALBUM_SORTS[..]
    }
    .iter()
    .map(|(v, t)| (v.to_string(), t.to_string()))
    .collect();
    let sort = if b.sort.is_empty() {
        sorts[0].0.clone()
    } else {
        b.sort.clone()
    };
    let any = || vec![(String::new(), "Any".to_string())];
    let mut codec_options = any();
    codec_options.extend(codecs.iter().map(|c| (c.clone(), c.clone())));
    let mut genre_options = any();
    genre_options.extend(genres.iter().map(|g| (g.clone(), g.clone())));
    // A filter set by a link that is not among the offered values still shows.
    for (current, options) in [
        (&b.codec, &mut codec_options),
        (&b.genre, &mut genre_options),
    ] {
        if let Some(c) = set(current)
            && !options.iter().any(|(v, _)| v == c)
        {
            options.push((c.to_owned(), c.to_owned()));
        }
    }
    let seed = match (sort.as_str(), b.seed) {
        ("random", Some(seed)) => format!("<input type=hidden name=seed value={seed}>"),
        _ => String::new(),
    };
    let reshuffle = if sort == "random" && !artists {
        let fresh = Browse {
            seed: None,
            ..b.clone()
        };
        format!(
            "<a class=\"text-muted\" href=\"{path}?{}\">Reshuffle</a>",
            fresh.query(0)
        )
    } else {
        String::new()
    };
    let active = b.active();
    let label = if active > 0 {
        format!(
            "Sort · Filter <span class=\"inline-block min-w-[1.5em] rounded-full bg-brand px-[5px] text-center \
text-[11px] font-bold text-bg\">{active}</span>"
        )
    } else {
        "Sort · Filter".into()
    };
    format!(
        "<details class=\"browse group -mt-1 mb-5\"><summary class=\"hidden cursor-pointer list-none items-center gap-1.5 \
rounded-md border border-rule bg-surface px-3 py-[7px] text-[14px] text-ink group-open:border-brand \
max-wide:inline-flex [&::-webkit-details-marker]:hidden\">{label}</summary>\
<form class=\"toolbar flex flex-wrap items-center gap-x-3.5 gap-y-2 text-[13px] text-muted max-wide:mt-2.5 \
max-wide:flex-col max-wide:items-stretch max-wide:gap-3 max-wide:rounded-[10px] max-wide:border \
max-wide:border-rule max-wide:bg-surface max-wide:p-3.5 max-wide:text-[15px]\" method=get action=\"{path}\">\
<label class=\"{LABEL}\">Name<input class=\"w-[12em] {FIELD} max-wide:w-auto max-wide:flex-1\" type=search \
name=q placeholder=\"{name_hint}\" value=\"{q}\" aria-label=\"Filter by name\"></label>{sort_select}{seed}{reshuffle}{fav}{lossless}{codec}\
<label class=\"{LABEL}\">Years<input class=\"w-[4.5em] {FIELD}\" name=from inputmode=numeric maxlength=4 \
placeholder=From value=\"{from}\" aria-label=\"From year\"><span>–</span><input class=\"w-[4.5em] {FIELD}\" \
name=to inputmode=numeric maxlength=4 placeholder=To value=\"{to}\" aria-label=\"To year\"></label>\
{genre}<div class=\"inline-flex items-center gap-2.5 max-wide:justify-between\">\
<button class=\"primary px-3 py-[5px] in-[.js]:hidden max-wide:in-[.js]:inline-block\">Apply</button>\
<a class=\"text-muted\" href=\"{path}\">Reset</a></div></form></details>",
        sort_select = select("sort", "Sort", &sorts, &sort),
        fav = check("fav", "Favourites", &b.fav),
        lossless = check("lossless", "Lossless", &b.lossless),
        codec = select("codec", "Codec", &codec_options, &b.codec),
        genre = select("genre", "Genre", &genre_options, &b.genre),
        name_hint = if artists { "Artist" } else { "Album or artist" },
        q = escape(&b.q),
        from = escape(&b.from),
        to = escape(&b.to),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browse(q: &str) -> Browse {
        let uri: axum::http::Uri = format!("/albums?{q}").parse().unwrap();
        axum::extract::Query::<Browse>::try_from_uri(&uri)
            .unwrap()
            .0
    }

    #[test]
    fn blank_fields_are_unset_and_the_query_round_trips() {
        let b =
            browse("sort=year&q=+aphex+&fav=&lossless=1&codec=&from=1990&to=&genre=Drum+%26+Bass");
        let q = b.albums(0, 60);
        assert_eq!(q.search, Some("aphex"));
        assert_eq!(b.artists(0, 60).search, Some("aphex"));
        assert_eq!(q.order, AlbumOrder::YearDesc);
        assert!(q.favourites_of.is_none() && q.filter.lossless);
        assert_eq!(
            (q.filter.codec, q.filter.year_from, q.filter.year_to),
            (None, Some(1990), None)
        );
        assert_eq!(q.filter.genre, Some("Drum & Bass"));
        assert_eq!(
            b.query(60),
            "sort=year&q=aphex&lossless=1&from=1990&genre=Drum+%26+Bass&offset=60"
        );
        assert_eq!(b.active(), 4);
    }

    #[test]
    fn a_random_order_keeps_its_seed_and_hostile_values_stay_encoded() {
        let b = browse("sort=random").seeded();
        let seed = b.seed.unwrap();
        assert_eq!(b.albums(0, 1).order, AlbumOrder::Random(seed));
        assert!(b.query(0).contains(&format!("seed={seed}")));
        let evil = browse("genre=%27%29%3Balert(1)%2F%2F%22%3E%3C");
        let q = evil.query(0);
        assert!(
            !q.contains('\'') && !q.contains('"') && !q.contains('<'),
            "{q}"
        );
    }
}
