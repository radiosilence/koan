use koan_core::db::queries;
use owo_colors::OwoColorize;

use super::{format_time, open_db};

pub fn cmd_search(query: &str) {
    let db = open_db();
    match queries::search_tracks(&db.conn, query) {
        Ok(tracks) => {
            if tracks.is_empty() {
                println!("no results for {}", format!("\"{}\"", query).dimmed());
                return;
            }
            println!(
                "{} results for {}\n",
                tracks.len().to_string().bold(),
                format!("\"{}\"", query).dimmed()
            );

            // Group tracks by artist -> album for tree display.
            struct AlbumGroup {
                title: String,
                album_id: Option<i64>,
                codec: Option<String>,
                has_local: bool,
                has_remote: bool,
                tracks: Vec<queries::TrackRow>,
            }
            struct ArtistGroup {
                name: String,
                albums: Vec<AlbumGroup>,
            }

            let mut artists: Vec<ArtistGroup> = Vec::new();

            for t in tracks {
                let artist = artists.iter_mut().find(|a| a.name == t.artist_name);
                let artist = match artist {
                    Some(a) => a,
                    None => {
                        artists.push(ArtistGroup {
                            name: t.artist_name.clone(),
                            albums: Vec::new(),
                        });
                        artists.last_mut().unwrap()
                    }
                };

                let album = artist
                    .albums
                    .iter_mut()
                    .find(|a| a.title == t.album_title && a.album_id == t.album_id);
                let album = match album {
                    Some(a) => a,
                    None => {
                        artist.albums.push(AlbumGroup {
                            title: t.album_title.clone(),
                            album_id: t.album_id,
                            codec: t.codec.clone(),
                            has_local: false,
                            has_remote: false,
                            tracks: Vec::new(),
                        });
                        artist.albums.last_mut().unwrap()
                    }
                };

                if t.path.is_some() {
                    album.has_local = true;
                }
                if t.remote_id.is_some() {
                    album.has_remote = true;
                }
                album.tracks.push(t);
            }

            // Render tree.
            for (ai, artist) in artists.iter().enumerate() {
                let is_last_artist = ai == artists.len() - 1;
                println!("{}", artist.name.bold().cyan());

                for (ali, album) in artist.albums.iter().enumerate() {
                    let is_last_album = ali == artist.albums.len() - 1;
                    let branch = if is_last_album {
                        "\u{2514}\u{2500}\u{2500} "
                    } else {
                        "\u{251c}\u{2500}\u{2500} "
                    };

                    let codec_tag = album
                        .codec
                        .as_deref()
                        .map(|c| format!(" [{}]", c).yellow().dimmed().to_string())
                        .unwrap_or_default();
                    let source_tag = match (album.has_local, album.has_remote) {
                        (true, true) => format!(" {}", "[local+remote]".magenta().dimmed()),
                        (false, true) => format!(" {}", "[remote]".magenta().dimmed()),
                        _ => String::new(),
                    };
                    let album_id = album
                        .album_id
                        .map(|id| format!(" {}", format!("[album:{}]", id).dimmed()))
                        .unwrap_or_default();

                    println!(
                        "{}{}{}{}{}",
                        branch.dimmed(),
                        album.title.green(),
                        codec_tag,
                        source_tag,
                        album_id,
                    );

                    let pipe = if is_last_album { "    " } else { "\u{2502}   " };
                    for (ti, t) in album.tracks.iter().enumerate() {
                        let is_last_track = ti == album.tracks.len() - 1;
                        let track_branch = if is_last_track {
                            "\u{2514}\u{2500}\u{2500} "
                        } else {
                            "\u{251c}\u{2500}\u{2500} "
                        };

                        let disc_track = match (t.disc, t.track_number) {
                            (Some(d), Some(n)) if d > 1 => format!("{}.{:02}", d, n),
                            (_, Some(n)) => format!("{:02}", n),
                            _ => "  ".into(),
                        };

                        let dur = t
                            .duration_ms
                            .map(|d| format_time(d as u64))
                            .unwrap_or_default();

                        println!(
                            "{}{}{} {} {}  {}",
                            pipe.dimmed(),
                            track_branch.dimmed(),
                            disc_track.dimmed(),
                            format!("[{}]", t.id).dimmed(),
                            t.title,
                            dur.dimmed(),
                        );
                    }
                }
                if !is_last_artist {
                    println!();
                }
            }
        }
        Err(e) => {
            eprintln!("{} {}", "search failed:".red().bold(), e);
            std::process::exit(1);
        }
    }
}
