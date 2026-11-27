//! Rows as response nodes, and the OpenSubsonic fields the row types do not
//! carry, read once per response for every entity in it.

use super::*;

/// A track as a `Child` element.
///
/// The tag varies by context — `song` in most responses, `entry` inside a
/// playlist, `child` inside a music directory — while the attributes do not.
/// The OpenSubsonic fields koan has data for are always present, empty when a
/// track has no value, so a client can tell a supported field from a missing
/// one; `played` is the exception, absent until the track has been played.
pub(super) fn track_node(track: &queries::TrackRow, tag: &str, extras: &SongExtras) -> XmlNode {
    let duration_secs = track.duration_ms.map(|ms| ms / 1000);
    let (suffix, content_type) = track
        .codec
        .as_deref()
        .map(codec_to_mime)
        .unwrap_or(("bin", "application/octet-stream"));
    let node = XmlNode::new(tag)
        .attr("id", &extras.uids.track(track.id))
        .attr("title", &track.title)
        .attr("album", &track.album_title)
        .attr("artist", &track.artist_name)
        .attr_opt_int("track", track.track_number.map(i64::from))
        .attr_opt_int("discNumber", track.disc.map(i64::from))
        .attr_opt_int("duration", duration_secs)
        .attr_opt_int("bitRate", track.bitrate.map(i64::from))
        .attr_opt("suffix", Some(suffix))
        .attr_opt("contentType", Some(content_type))
        // koan's own: the suffix cannot tell ALAC from AAC, both being m4a.
        .attr_opt("codec", track.codec.as_deref())
        .attr_opt("genre", track.genre.as_deref())
        .attr_opt(
            "albumId",
            track.album_id.map(|id| extras.uids.album(id)).as_deref(),
        )
        .attr_opt(
            "artistId",
            track.artist_id.map(|id| extras.uids.artist(id)).as_deref(),
        )
        .attr_opt(
            "parent",
            track.album_id.map(|id| extras.uids.album(id)).as_deref(),
        )
        .attr("coverArt", &extras.uids.track(track.id))
        .attr("type", "music")
        .attr_bool("isDir", false)
        .attr("mediaType", "song")
        .attr_int("bitDepth", track.bit_depth.unwrap_or(0).into())
        .attr_int("samplingRate", track.sample_rate.unwrap_or(0).into())
        .attr_int("channelCount", track.channels.unwrap_or(0).into())
        .attr("displayArtist", &track.artist_name)
        .attr("displayAlbumArtist", &track.album_artist_name)
        .attr(
            "musicBrainzId",
            extras.mbid.get(&track.id).map_or("", String::as_str),
        )
        .attr_opt(
            "path",
            extras
                .library
                .as_deref()
                .zip(track.path.as_deref())
                .map(|(folders, path)| library_path(path, folders))
                .as_deref(),
        )
        .attr_opt(
            "played",
            extras.played.get(&track.id).map(|&at| iso(at)).as_deref(),
        )
        .attr_opt(
            "starred",
            extras.starred.get(&track.id).map(|&at| iso(at)).as_deref(),
        )
        .attr_opt_int(
            "userRating",
            extras.rating.get(&track.id).map(|&r| r.into()),
        )
        .list("genres", track.genre.iter().map(|g| genre_node(g)))
        .list(
            "artists",
            credited_or_filed(
                extras.credits.get(&track.id),
                track.artist_id.map(|id| (id, track.artist_name.as_str())),
                &extras.uids,
            ),
        );
    // Only where the album credits its artists one by one: an album filed
    // under one artist names it in `artist` already.
    let album_artists = credit_refs(
        "albumArtists",
        track.album_id.and_then(|a| extras.album_credits.get(&a)),
        &extras.uids,
    );
    if album_artists.is_empty() {
        node
    } else {
        node.list("albumArtists", album_artists)
    }
}

pub(super) fn track_to_xml_node(track: &queries::TrackRow, extras: &SongExtras) -> XmlNode {
    track_node(track, "song", extras)
}

pub(super) fn genre_node(name: &str) -> XmlNode {
    XmlNode::new("genres").attr("name", name)
}

/// An `ArtistID3` inside a list field, with only its required fields.
pub(super) fn artist_ref(tag: &str, id: &str, name: &str) -> XmlNode {
    XmlNode::new(tag).attr("id", id).attr("name", name)
}

pub(super) fn year_from_date(date: Option<&str>) -> Option<i64> {
    date.and_then(|d| d.get(..4)).and_then(|y| y.parse().ok())
}

/// An OpenSubsonic `ItemDate` from a tag date: `2020`, `2020-05` or
/// `2020-05-17`, each part given only when it and those before it parse.
pub(super) fn item_date(tag: &str, date: Option<&str>) -> XmlNode {
    let mut node = XmlNode::new(tag);
    let date = date.unwrap_or_default();
    let fields = ["year", "month", "day"]
        .into_iter()
        .zip([1..=9999, 1..=12, 1..=31]);
    for ((key, range), part) in fields.zip(date.get(..10).unwrap_or(date).split('-')) {
        match part.parse::<i64>() {
            Ok(n) if range.contains(&n) => node = node.attr_int(key, n),
            _ => break,
        }
    }
    node
}

/// An album as an `AlbumID3` element. `title` rides alongside `name` because
/// the file-browse half of the protocol spells it that way and clients mix the
/// two freely. koan keeps one date per album, a tag's release or recording
/// date, and gives it as `releaseDate`.
pub(super) fn album_to_xml_node(album: &queries::AlbumRow, extras: &AlbumExtras) -> XmlNode {
    let stats = extras.stats.get(&album.id).copied().unwrap_or_default();
    let (mbid, sort_name) = extras
        .names
        .get(&album.id)
        .map(|(m, s)| (m.as_deref(), s.as_deref()))
        .unwrap_or_default();
    let genres = extras
        .genres
        .get(&album.id)
        .map(Vec::as_slice)
        .unwrap_or_default();
    XmlNode::new("album")
        .attr("id", &extras.uids.album(album.id))
        .attr("name", &album.title)
        .attr("title", &album.title)
        .attr("artist", &album.artist_name)
        .attr("artistId", &extras.uids.artist(album.artist_id))
        .attr("parent", &extras.uids.artist(album.artist_id))
        .attr("coverArt", &extras.uids.album(album.id))
        .attr_int("songCount", stats.track_count)
        .attr_int("duration", stats.total_duration_ms / 1000)
        .attr_opt("created", album.added_at.as_deref())
        .attr_opt_int("year", year_from_date(album.date.as_deref()))
        .attr_opt("genre", genres.first().map(String::as_str))
        .attr_bool("isDir", true)
        .attr("musicBrainzId", mbid.unwrap_or_default())
        .attr("sortName", sort_name.unwrap_or_default())
        .attr("displayArtist", &album.artist_name)
        .attr_opt(
            "played",
            extras.played.get(&album.id).map(|&at| iso(at)).as_deref(),
        )
        .attr_opt(
            "starred",
            extras.starred.get(&album.id).map(|&at| iso(at)).as_deref(),
        )
        .attr_opt_int(
            "userRating",
            extras.rating.get(&album.id).map(|&r| r.into()),
        )
        .child(item_date("releaseDate", album.date.as_deref()))
        .list("genres", genres.iter().map(|g| genre_node(g)))
        .list(
            "artists",
            credited_or_filed(
                extras.credits.get(&album.id),
                Some((album.artist_id, album.artist_name.as_str())),
                &extras.uids,
            ),
        )
        .list(
            "recordLabels",
            album
                .label
                .iter()
                .map(|l| XmlNode::new("recordLabels").attr("name", l)),
        )
        .list(
            "releaseTypes",
            album
                .release_types
                .iter()
                .flat_map(|t| t.split(';'))
                .map(|t| XmlNode::scalar("releaseTypes", AttrValue::Str(t.into()))),
        )
}

/// An artist as an `ArtistID3` element, without `albumCount`, which each
/// caller knows its own way.
pub(super) fn artist_id3_node(id: i64, name: &str, extras: &ArtistExtras) -> XmlNode {
    let (mbid, sort_name) = extras
        .names
        .get(&id)
        .map(|(m, s)| (m.as_deref(), s.as_deref()))
        .unwrap_or_default();
    let uid = extras.uids.artist(id);
    XmlNode::new("artist")
        .attr("id", &uid)
        .attr("name", name)
        .attr("coverArt", &uid)
        .attr("musicBrainzId", mbid.unwrap_or_default())
        .attr("sortName", sort_name.unwrap_or_default())
        .attr_opt_int("userRating", extras.rating.get(&id).map(|&r| r.into()))
        .attr_opt(
            "starred",
            extras.starred.get(&id).map(|&at| iso(at)).as_deref(),
        )
}

/// Ids as one JSON array, for `IN (SELECT value FROM json_each(?1))` — a
/// single bound parameter however long the list.
pub(super) fn json_ids(ids: impl IntoIterator<Item = i64>) -> String {
    serde_json::to_string(&ids.into_iter().collect::<Vec<_>>()).unwrap_or_default()
}

pub(super) fn by_id<T>(
    db: &Database,
    sql: &str,
    params: impl rusqlite::Params,
    mut row: impl FnMut(&rusqlite::Row) -> rusqlite::Result<(i64, T)>,
) -> Result<Vec<(i64, T)>, SubsonicError> {
    let internal = |e: rusqlite::Error| SubsonicError::internal(e.to_string());
    let mut stmt = db.conn.prepare_cached(sql).map_err(internal)?;
    stmt.query_map(params, |r| row(r))
        .map_err(internal)?
        .collect::<Result<_, _>>()
        .map_err(internal)
}

/// What `Child` carries beyond `TrackRow`.
#[derive(Default)]
pub(super) struct SongExtras {
    pub(super) uids: Uids,
    /// Each artist a track's credits name, by track, and its album's, by album.
    pub(super) credits: HashMap<i64, Vec<Credited>>,
    pub(super) album_credits: HashMap<i64, Vec<Credited>>,
    pub(super) mbid: HashMap<i64, String>,
    /// Last play, seconds since the epoch.
    pub(super) played: HashMap<i64, i64>,
    /// The caller's rating, 1 to 5.
    pub(super) rating: HashMap<i64, u8>,
    /// When the caller favourited it, seconds since the epoch.
    pub(super) starred: HashMap<i64, i64>,
    /// The library folders, for an admin: songs then carry `path`. Paths
    /// describe the server's filesystem, which is no one else's business.
    pub(super) library: Option<Vec<PathBuf>>,
}

/// The caller's own history: when someone else last played a track is theirs
/// to know.
pub(super) fn history_user(db: &Database, user: i64) -> Result<i64, SubsonicError> {
    queries::auth::resolve_user(&db.conn, user).map_err(|e| SubsonicError::internal(e.to_string()))
}

pub(super) fn song_extras<'a>(
    db: &Database,
    caller: &Caller,
    tracks: impl IntoIterator<Item = &'a queries::TrackRow>,
) -> Result<SongExtras, SubsonicError> {
    let tracks: Vec<_> = tracks.into_iter().collect();
    if tracks.is_empty() {
        return Ok(SongExtras::default());
    }
    let ids = json_ids(tracks.iter().map(|t| t.id));
    let user = history_user(db, caller.user_id)?;
    let credits = credits_of(db, CreditsOf::Tracks, tracks.iter().map(|t| t.id))?;
    let album_credits = credits_of(
        db,
        CreditsOf::Albums,
        tracks.iter().filter_map(|t| t.album_id),
    )?;
    let credited = credits
        .values()
        .chain(album_credits.values())
        .flatten()
        .map(|c| c.artist.id)
        .collect::<Vec<_>>();
    Ok(SongExtras {
        uids: Uids::load(
            db,
            tracks.iter().filter_map(|t| t.artist_id).chain(credited),
            tracks.iter().filter_map(|t| t.album_id),
            tracks.iter().map(|t| t.id),
        )?,
        credits,
        album_credits,
        mbid: by_id(
            db,
            "SELECT id, mbid FROM tracks
             WHERE id IN (SELECT value FROM json_each(?1)) AND mbid IS NOT NULL",
            [&ids],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .into_iter()
        .collect(),
        played: by_id(
            db,
            "SELECT track_id, MAX(played_at) FROM play_history
             WHERE track_id IN (SELECT value FROM json_each(?1)) AND user_id = ?2
             GROUP BY track_id",
            rusqlite::params![ids, user],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .into_iter()
        .collect(),
        rating: rated(
            db,
            user,
            queries::RatingKind::Track,
            tracks.iter().map(|t| t.id),
        )?,
        starred: starred(db, user, Favourite::Track, &ids)?,
        library: (caller.role == Role::Admin).then(|| {
            Config::cached()
                .library
                .folders
                .iter()
                .map(|f| koan_core::index::spelling::on_disk(f))
                .collect()
        }),
    })
}

use queries::batch::{CreditedArtist as Credited, CreditsOf};

/// Each artist the credits of these tracks or albums name, by owner.
pub(super) fn credits_of(
    db: &Database,
    of: CreditsOf,
    ids: impl IntoIterator<Item = i64>,
) -> Result<HashMap<i64, Vec<Credited>>, SubsonicError> {
    let ids: Vec<i64> = ids.into_iter().collect();
    queries::batch::credited_artists(&db.conn, of, &ids)
        .map_err(|e| SubsonicError::internal(e.to_string()))
}

/// A song's or album's `artists`: each credited artist where its credits name
/// them, otherwise the one it is filed under.
pub(super) fn credited_or_filed(
    credits: Option<&Vec<Credited>>,
    filed: Option<(i64, &str)>,
    uids: &Uids,
) -> Vec<XmlNode> {
    let credited = credit_refs("artists", credits, uids);
    if !credited.is_empty() {
        return credited;
    }
    filed
        .map(|(id, name)| artist_ref("artists", &uids.artist(id), name))
        .into_iter()
        .collect()
}

/// Each credited artist of a song or album, as `tag` elements.
pub(super) fn credit_refs(tag: &str, credits: Option<&Vec<Credited>>, uids: &Uids) -> Vec<XmlNode> {
    credits
        .into_iter()
        .flatten()
        .map(|c| {
            artist_ref(tag, &uids.artist(c.artist.id), &c.artist.name)
                .attr_opt("musicBrainzId", c.mbid.as_deref())
        })
        .collect()
}

/// The caller's ratings of these rows.
pub(super) fn rated(
    db: &Database,
    user: i64,
    kind: queries::RatingKind,
    ids: impl IntoIterator<Item = i64>,
) -> Result<HashMap<i64, u8>, SubsonicError> {
    queries::ratings(&db.conn, user, kind, ids).map_err(|e| SubsonicError::internal(e.to_string()))
}

pub(super) enum Favourite {
    Track,
    Album,
    Artist,
}

/// When the caller favourited these rows (`ids` as from [`json_ids`]), seconds
/// since the epoch. `user` is already resolved.
pub(super) fn starred(
    db: &Database,
    user: i64,
    kind: Favourite,
    ids: &str,
) -> Result<HashMap<i64, i64>, SubsonicError> {
    let sql = match kind {
        Favourite::Track => {
            "SELECT track_id, COALESCE(unixepoch(created_at), 0) FROM favourites
             WHERE track_id IN (SELECT value FROM json_each(?1)) AND user_id = ?2"
        }
        Favourite::Album => {
            "SELECT album_id, COALESCE(unixepoch(created_at), 0) FROM favourite_albums
             WHERE album_id IN (SELECT value FROM json_each(?1)) AND user_id = ?2"
        }
        Favourite::Artist => {
            "SELECT artist_id, COALESCE(unixepoch(created_at), 0) FROM favourite_artists
             WHERE artist_id IN (SELECT value FROM json_each(?1)) AND user_id = ?2"
        }
    };
    Ok(by_id(db, sql, rusqlite::params![ids, user], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?
    .into_iter()
    .collect())
}

/// What `AlbumID3` carries beyond `AlbumRow`.
#[derive(Default)]
pub(super) struct AlbumExtras {
    pub(super) uids: Uids,
    pub(super) credits: HashMap<i64, Vec<Credited>>,
    /// MusicBrainz release id and sort name.
    pub(super) names: HashMap<i64, (Option<String>, Option<String>)>,
    pub(super) genres: HashMap<i64, Vec<String>>,
    pub(super) stats: HashMap<i64, queries::AlbumStats>,
    pub(super) played: HashMap<i64, i64>,
    pub(super) rating: HashMap<i64, u8>,
    pub(super) starred: HashMap<i64, i64>,
}

/// `tracks`, when the caller already read every track of these albums, is
/// where their genres and totals come from instead of the database.
pub(super) fn album_extras<'a>(
    db: &Database,
    user: i64,
    albums: impl IntoIterator<Item = &'a queries::AlbumRow>,
    tracks: Option<&[queries::TrackRow]>,
) -> Result<AlbumExtras, SubsonicError> {
    let albums: Vec<_> = albums.into_iter().collect();
    if albums.is_empty() {
        return Ok(AlbumExtras::default());
    }
    let album_ids: Vec<i64> = albums.iter().map(|a| a.id).collect();
    let ids = json_ids(album_ids.iter().copied());
    let user = history_user(db, user)?;
    let (genres, stats) = match tracks {
        Some(tracks) => album_totals(tracks),
        None => {
            let mut genres: HashMap<i64, Vec<String>> = HashMap::new();
            for (id, genre) in by_id(
                db,
                "SELECT DISTINCT album_id, genre FROM tracks
                 WHERE album_id IN (SELECT value FROM json_each(?1))
                   AND genre IS NOT NULL AND genre != ''
                 ORDER BY album_id, genre",
                [&ids],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )? {
                genres.entry(id).or_default().push(genre);
            }
            let stats = queries::album_stats(&db.conn, &album_ids)
                .map_err(|e| SubsonicError::internal(e.to_string()))?;
            (genres, stats)
        }
    };
    let credits = credits_of(db, CreditsOf::Albums, album_ids.iter().copied())?;
    let credited = credits
        .values()
        .flatten()
        .map(|c| c.artist.id)
        .collect::<Vec<_>>();
    Ok(AlbumExtras {
        uids: Uids::load(
            db,
            albums.iter().map(|a| a.artist_id).chain(credited),
            album_ids.iter().copied(),
            [],
        )?,
        credits,
        names: by_id(
            db,
            "SELECT id, mbid, sort_name FROM albums WHERE id IN (SELECT value FROM json_each(?1))",
            [&ids],
            |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))),
        )?
        .into_iter()
        .collect(),
        genres,
        stats,
        played: by_id(
            db,
            "SELECT t.album_id, MAX(h.played_at) FROM play_history h
             JOIN tracks t ON t.id = h.track_id
             WHERE t.album_id IN (SELECT value FROM json_each(?1)) AND h.user_id = ?2
             GROUP BY t.album_id",
            rusqlite::params![ids, user],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .into_iter()
        .collect(),
        rating: rated(
            db,
            user,
            queries::RatingKind::Album,
            album_ids.iter().copied(),
        )?,
        starred: starred(db, user, Favourite::Album, &ids)?,
    })
}

/// Each album's genres, sorted and distinct, and its track count and running
/// time, from its tracks. What `album_extras` would otherwise ask the database.
pub(super) fn album_totals(
    tracks: &[queries::TrackRow],
) -> (HashMap<i64, Vec<String>>, HashMap<i64, queries::AlbumStats>) {
    let mut genres: HashMap<i64, std::collections::BTreeSet<String>> = HashMap::new();
    let mut stats: HashMap<i64, queries::AlbumStats> = HashMap::new();
    for t in tracks {
        let Some(album) = t.album_id else { continue };
        let s = stats.entry(album).or_default();
        s.track_count += 1;
        s.total_duration_ms += t.duration_ms.unwrap_or(0);
        if let Some(genre) = t.genre.as_ref().filter(|g| !g.is_empty()) {
            genres.entry(album).or_default().insert(genre.clone());
        }
    }
    let genres = genres
        .into_iter()
        .map(|(id, g)| (id, g.into_iter().collect()))
        .collect();
    (genres, stats)
}

/// What `ArtistID3` carries beyond a name.
pub(super) struct ArtistExtras {
    pub(super) uids: Uids,
    /// MusicBrainz artist id and sort name.
    pub(super) names: HashMap<i64, (Option<String>, Option<String>)>,
    pub(super) rating: HashMap<i64, u8>,
    pub(super) starred: HashMap<i64, i64>,
}

pub(super) fn artist_extras(
    db: &Database,
    user: i64,
    ids: impl IntoIterator<Item = i64>,
) -> Result<ArtistExtras, SubsonicError> {
    let ids: Vec<i64> = ids.into_iter().collect();
    let json = json_ids(ids.iter().copied());
    Ok(ArtistExtras {
        uids: Uids::load(db, ids.iter().copied(), [], [])?,
        rating: rated(db, user, queries::RatingKind::Artist, ids.iter().copied())?,
        starred: starred(db, history_user(db, user)?, Favourite::Artist, &json)?,
        names: by_id(
            db,
            "SELECT id, mbid, sort_name FROM artists WHERE id IN (SELECT value FROM json_each(?1))",
            [&json],
            |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))),
        )?
        .into_iter()
        .collect(),
    })
}

/// An album as a directory `child`, for the file-browse endpoints, with the
/// caller's own marks on it as `AlbumID3` carries them.
pub(super) fn album_child_node(album: &queries::AlbumRow, extras: &AlbumExtras) -> XmlNode {
    let uids = &extras.uids;
    XmlNode::new("child")
        .attr("id", &uids.album(album.id))
        .attr("parent", &uids.artist(album.artist_id))
        .attr("title", &album.title)
        .attr("album", &album.title)
        .attr("artist", &album.artist_name)
        .attr("coverArt", &uids.album(album.id))
        .attr_opt_int("year", year_from_date(album.date.as_deref()))
        .attr_bool("isDir", true)
        .attr_opt(
            "played",
            extras.played.get(&album.id).map(|&at| iso(at)).as_deref(),
        )
        .attr_opt(
            "starred",
            extras.starred.get(&album.id).map(|&at| iso(at)).as_deref(),
        )
        .attr_opt_int(
            "userRating",
            extras.rating.get(&album.id).map(|&r| r.into()),
        )
}

/// Artists bucketed by first letter — the shape `getArtists` (ID3) and
/// `getIndexes` (file-browse) both hang off.
pub(super) type ArtistIndex = BTreeMap<String, Vec<queries::ArtistRow>>;

pub(super) fn artist_index(db: &Database) -> Result<ArtistIndex, SubsonicError> {
    // Neither listing shows a track count.
    let artists = queries::list_artists(
        &db.conn,
        &queries::ArtistQuery {
            without_track_counts: true,
            ..Default::default()
        },
    )
    .map_err(|e| SubsonicError::internal(e.to_string()))?;

    let mut index_map: BTreeMap<String, Vec<queries::ArtistRow>> = BTreeMap::new();
    for artist in artists {
        let letter = artist
            .sort_name
            .as_deref()
            .unwrap_or(&artist.name)
            .chars()
            .next()
            .map(|c| {
                let upper = c.to_uppercase().to_string();
                if upper
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_alphabetic())
                {
                    upper
                } else {
                    "#".to_string()
                }
            })
            .unwrap_or_else(|| "#".to_string());
        index_map.entry(letter).or_default().push(artist);
    }

    Ok(index_map)
}

/// A stored codec as a Subsonic `suffix` and `contentType`.
///
/// Covers both what the indexer stores (`index::metadata`) and the suffixes a
/// track synced from another Subsonic server arrives with.
pub(super) fn codec_to_mime(codec: &str) -> (&str, &str) {
    match codec.to_uppercase().as_str() {
        "FLAC" => ("flac", "audio/flac"),
        "MP3" => ("mp3", "audio/mpeg"),
        "AAC" | "ALAC" | "M4A" | "MP4" => ("m4a", "audio/mp4"),
        "OPUS" => ("opus", "audio/opus"),
        "VORBIS" | "OGG" | "OGA" => ("ogg", "audio/ogg"),
        "WAV" | "PCM" => ("wav", "audio/wav"),
        "AIFF" | "AIF" => ("aiff", "audio/aiff"),
        "APE" => ("ape", "audio/x-ape"),
        _ => ("bin", "application/octet-stream"),
    }
}

pub(crate) fn extension_to_mime(ext: &str) -> &str {
    match ext.to_lowercase().as_str() {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" | "mp4" | "alac" => "audio/mp4",
        "opus" => "audio/opus",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
        "ape" => "audio/x-ape",
        _ => "application/octet-stream",
    }
}

/// A file as Subsonic's `path`: relative to the library folder that holds it,
/// as Navidrome sends it, or whole for a file imported from elsewhere.
/// `folders` are spelled as on disk, as the indexed paths are.
pub(super) fn library_path(path: &str, folders: &[PathBuf]) -> String {
    let path = std::path::Path::new(path);
    folders
        .iter()
        .filter_map(|folder| path.strip_prefix(folder).ok())
        .filter(|rest| !rest.as_os_str().is_empty())
        .min_by_key(|rest| rest.components().count())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Resolve a track's file path (local preferred, then cached).
pub(crate) fn track_file_path(track: &queries::TrackRow) -> Option<&str> {
    track.path.as_deref().or(track.cached_path.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::testing::*;

    #[test]
    fn test_codec_to_mime() {
        assert_eq!(codec_to_mime("FLAC"), ("flac", "audio/flac"));
        assert_eq!(codec_to_mime("MP3"), ("mp3", "audio/mpeg"));
        assert_eq!(codec_to_mime("AAC"), ("m4a", "audio/mp4"));
        assert_eq!(codec_to_mime("Opus"), ("opus", "audio/opus"));
        assert_eq!(codec_to_mime("ALAC"), ("m4a", "audio/mp4"));
        assert_eq!(codec_to_mime("Vorbis"), ("ogg", "audio/ogg"));
        assert_eq!(codec_to_mime("PCM"), ("wav", "audio/wav"));
        assert_eq!(codec_to_mime("WAV"), ("wav", "audio/wav"));
        assert_eq!(codec_to_mime("AIFF"), ("aiff", "audio/aiff"));
        assert_eq!(codec_to_mime("aif"), ("aiff", "audio/aiff"));
        assert_eq!(codec_to_mime("APE"), ("ape", "audio/x-ape"));
        assert_eq!(
            codec_to_mime("Unknown"),
            ("bin", "application/octet-stream")
        );
    }

    #[test]
    fn test_extension_to_mime() {
        assert_eq!(extension_to_mime("flac"), "audio/flac");
        assert_eq!(extension_to_mime("mp3"), "audio/mpeg");
        assert_eq!(extension_to_mime("m4a"), "audio/mp4");
        assert_eq!(extension_to_mime("FLAC"), "audio/flac");
    }

    #[test]
    fn a_library_path_is_relative_to_the_folder_holding_it() {
        let folders = [PathBuf::from("/music"), PathBuf::from("/music/classical")];
        let path = |p| library_path(p, &folders);
        assert_eq!(path("/music/classical/Bach/01.flac"), "Bach/01.flac");
        assert_eq!(path("/music/Burial/01.flac"), "Burial/01.flac");
        assert_eq!(path("/imported/01.flac"), "/imported/01.flac");
        assert_eq!(path("/music"), "/music", "never empty");
    }

    /// Typed attributes must not change the XML wire format: every attribute is
    /// still a quoted string.
    #[tokio::test]
    async fn test_song_xml_is_unchanged_by_typed_attributes() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let track = queries::all_tracks(&db.conn).unwrap()[0].clone();
        let id = uid_of(&state, queries::UidKind::Track, track.id);
        let album = uid_of(&state, queries::UidKind::Album, track.album_id.unwrap());
        let artist = uid_of(&state, queries::UidKind::Artist, track.artist_id.unwrap());

        let app = build_test_router(state);
        let (_, body) = get_response(
            app,
            &format!("/rest/getSong?{}&id={}", auth_query(""), track.id),
        )
        .await;

        let song = body
            .lines()
            .find(|l| l.trim_start().starts_with("<song "))
            .expect("no <song> element")
            .trim()
            .to_string();
        assert_eq!(
            song,
            format!(
                concat!(
                    r#"<song id="{id}" title="Test Song" album="Test Album" artist="Test Artist" "#,
                    r#"track="1" discNumber="1" duration="240" bitRate="1411" suffix="flac" "#,
                    r#"contentType="audio/flac" codec="FLAC" genre="Rock" albumId="{album}" artistId="{artist}" "#,
                    r#"parent="{album}" coverArt="{id}" type="music" isDir="false" "#,
                    r#"mediaType="song" bitDepth="16" samplingRate="44100" channelCount="2" "#,
                    r#"displayArtist="Test Artist" displayAlbumArtist="Test Artist" "#,
                    r#"musicBrainzId="">"#
                ),
                id = id,
                album = album,
                artist = artist,
            )
        );
    }

    /// Strictly-typed clients (Symfonium, Substreamer, Feishin) reject
    /// `"duration": "240"`, so numeric and boolean fields go out unquoted in JSON.
    #[tokio::test]
    async fn test_song_json_field_types() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct StrictSong {
            id: String,
            title: String,
            duration: i64,
            track: i64,
            bit_rate: i64,
            disc_number: i64,
            is_dir: bool,
            #[serde(rename = "type")]
            kind: String,
            cover_art: String,
        }

        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let track_id = queries::all_tracks(&db.conn).unwrap()[0].id;

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/getSong?{}&id={}", auth_query("f=json"), track_id),
        )
        .await;

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let song: StrictSong =
            serde_json::from_value(parsed["subsonic-response"]["song"].clone()).unwrap();

        let uid = uid_of(&state, queries::UidKind::Track, track_id);
        assert_eq!(song.id, uid);
        assert_eq!(song.title, "Test Song");
        assert_eq!(song.duration, 240);
        assert_eq!(song.track, 1);
        assert_eq!(song.bit_rate, 1411);
        assert_eq!(song.disc_number, 1);
        assert!(!song.is_dir);
        assert_eq!(song.kind, "music");
        assert_eq!(song.cover_art, uid);
    }

    #[tokio::test]
    async fn test_album_json_field_types() {
        let (state, _dir) = test_state();
        seed_data(&state);

        let db = Database::open(state.pool.path()).unwrap();
        let album_id = queries::all_albums(&db.conn).unwrap()[0].id;

        let app = build_test_router(state.clone());
        let (_, body) = get_response(
            app,
            &format!("/rest/getAlbum?{}&id={}", auth_query("f=json"), album_id),
        )
        .await;

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let album = &parsed["subsonic-response"]["album"];
        assert_eq!(album["songCount"], serde_json::json!(1));
        assert_eq!(album["year"], serde_json::json!(2020));
        assert_eq!(album["isDir"], serde_json::json!(true));
        assert_eq!(
            album["coverArt"],
            uid_of(&state, queries::UidKind::Album, album_id)
        );
    }

    /// An album's year is its original release's where its tags name one, and
    /// its release's otherwise, whole dates included; `byYear` lists them in
    /// that order.
    #[tokio::test]
    async fn an_albums_year_is_its_original_release() {
        let (state, _dir) = test_state();
        let db = Database::open(state.pool.path()).unwrap();
        for (i, (album, date, original)) in [
            ("Meddle", Some("2016"), Some("1971-10-30")),
            ("Piper", None, Some("1967")),
            ("The Wall", Some("1979-11-30"), None),
            ("Ummagumma", Some("1969"), None),
        ]
        .into_iter()
        .enumerate()
        {
            let mut meta = track_meta(&format!("/music/{i}.flac"), "Song", album, 1);
            meta.date = date.map(Into::into);
            meta.original_date = original.map(Into::into);
            queries::upsert_track(&db.conn, &meta).unwrap();
        }
        let r = json_of(
            build_test_router(state.clone()),
            &format!(
                "/rest/getAlbumList2?{}&type=byYear&fromYear=1900&toYear=2100",
                auth_query("f=json")
            ),
        )
        .await;
        let years: Vec<(String, i64)> = r["albumList2"]["album"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                (
                    a["name"].as_str().unwrap().to_string(),
                    a["year"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            years,
            vec![
                ("Piper".into(), 1967),
                ("Ummagumma".into(), 1969),
                ("Meddle".into(), 1971),
                ("The Wall".into(), 1979),
            ]
        );
    }

    #[tokio::test]
    async fn test_opensubsonic_fields() {
        let (state, _dir) = test_state();
        let db = Database::open(state.pool.path()).unwrap();
        let mut meta = track_meta("/music/a.flac", "Song", "Record", 1);
        meta.date = Some("2020-05-17".into());
        meta.mbid = Some("rec-mbid".into());
        meta.album_mbid = Some("rel-mbid".into());
        meta.label = Some("Warp".into());
        meta.release_types = Some("ep;live".into());
        queries::upsert_track(&db.conn, &meta).unwrap();
        let track = queries::all_tracks(&db.conn).unwrap().remove(0);
        let album_id = track.album_id.unwrap();
        let app = || build_test_router(state.clone());

        let r = json_of(
            app(),
            &format!("/rest/getAlbum?{}&id={album_id}", auth_query("f=json")),
        )
        .await;
        let album = &r["album"];
        assert_eq!(album["musicBrainzId"], "rel-mbid");
        assert_eq!(album["sortName"], "");
        assert_eq!(album["displayArtist"], "Test Artist");
        assert_eq!(album["songCount"], 1);
        assert_eq!(album["duration"], 240);
        assert_eq!(album["genre"], "Rock");
        assert_eq!(album["genres"], serde_json::json!([{"name": "Rock"}]));
        assert_eq!(album["recordLabels"], serde_json::json!([{"name": "Warp"}]));
        assert_eq!(album["releaseTypes"], serde_json::json!(["ep", "live"]));
        assert_eq!(
            album["releaseDate"],
            serde_json::json!({"year": 2020, "month": 5, "day": 17})
        );
        assert_eq!(album["artists"][0]["name"], "Test Artist");
        assert!(album.get("played").is_none());

        let song = &album["song"][0];
        assert_eq!(song["mediaType"], "song");
        assert_eq!(song["musicBrainzId"], "rec-mbid");
        assert_eq!(song["bitDepth"], 16);
        assert_eq!(song["samplingRate"], 44100);
        assert_eq!(song["channelCount"], 2);
        assert_eq!(song["displayAlbumArtist"], "Test Artist");
        assert_eq!(song["genres"], serde_json::json!([{"name": "Rock"}]));
        assert_eq!(song["artists"][0]["name"], "Test Artist");
        assert!(song.get("played").is_none());

        queries::record_play_at(
            &db.conn,
            koan_core::db::queries::LOCAL_USER,
            track.id,
            1_700_000_000,
            None,
            "local",
        )
        .unwrap();
        let r = json_of(
            app(),
            &format!("/rest/getSong?{}&id={}", auth_query("f=json"), track.id),
        )
        .await;
        assert_eq!(r["song"]["played"], "2023-11-14T22:13:20.000Z");
        let r = json_of(
            app(),
            &format!("/rest/getAlbum?{}&id={album_id}", auth_query("f=json")),
        )
        .await;
        assert_eq!(r["album"]["played"], "2023-11-14T22:13:20.000Z");

        // Another account's history is not this one's to see.
        let mate = "u=mate&p=hunter22&v=1.16.1&c=test&f=json";
        let r = json_of(app(), &format!("/rest/getSong?{mate}&id={}", track.id)).await;
        assert!(r["song"]["id"].is_string(), "{r}");
        assert!(r["song"].get("played").is_none(), "{r}");
        let r = json_of(app(), &format!("/rest/getAlbum?{mate}&id={album_id}")).await;
        assert!(r["album"]["id"].is_string(), "{r}");
        assert!(r["album"].get("played").is_none(), "{r}");

        let r = json_of(app(), &format!("/rest/getArtists?{}", auth_query("f=json"))).await;
        let artist = &r["artists"]["index"][0]["artist"][0];
        assert_eq!(artist["musicBrainzId"], "");
        assert_eq!(artist["sortName"], "");

        // A lone search hit is still a one-element array.
        let r = json_of(
            app(),
            &format!("/rest/search3?{}&query=Record", auth_query("f=json")),
        )
        .await;
        let found = &r["searchResult3"];
        assert!(
            found["song"].is_array() && found["album"].is_array(),
            "{found}"
        );
        assert_eq!(found["album"][0]["songCount"], 1);
    }
}
