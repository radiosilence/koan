use koan_core::config;
use koan_core::db::queries;
use owo_colors::OwoColorize;

use super::open_db;

pub fn cmd_artists(query: Option<&str>) {
    let db = open_db();
    let artists = if let Some(q) = query {
        queries::find_artists(&db.conn, q)
    } else {
        queries::all_artists(&db.conn)
    };

    match artists {
        Ok(artists) => {
            if artists.is_empty() {
                println!("no artists found");
                return;
            }
            for a in &artists {
                let remote_tag = if a.remote_id.is_some() {
                    format!(" {}", "[remote]".magenta().dimmed())
                } else {
                    String::new()
                };
                println!(
                    "  {} {}{}",
                    format!("[{}]", a.id).dimmed(),
                    a.name.bold().cyan(),
                    remote_tag,
                );
            }
        }
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

pub fn cmd_albums(query: Option<&str>) {
    let db = open_db();

    let albums = if let Some(q) = query {
        let artists = queries::find_artists(&db.conn, q).unwrap_or_default();
        if artists.is_empty() {
            println!("no artist matching {}", format!("\"{}\"", q).dimmed());
            return;
        }
        let mut all_albums = Vec::new();
        for a in &artists {
            if let Ok(mut albums) = queries::albums_for_artist(&db.conn, a.id) {
                all_albums.append(&mut albums);
            }
        }
        Ok(all_albums)
    } else {
        queries::all_albums(&db.conn)
    };

    match albums {
        Ok(albums) => {
            if albums.is_empty() {
                println!("no albums found");
                return;
            }
            let mut current_artist = String::new();
            let mut artist_albums: Vec<&queries::AlbumRow> = Vec::new();

            // Collect albums per artist for tree rendering.
            let mut grouped: Vec<(String, Vec<&queries::AlbumRow>)> = Vec::new();
            for al in &albums {
                if al.artist_name != current_artist {
                    if !artist_albums.is_empty() {
                        grouped.push((current_artist.clone(), artist_albums.clone()));
                    }
                    current_artist = al.artist_name.clone();
                    artist_albums = Vec::new();
                }
                artist_albums.push(al);
            }
            if !artist_albums.is_empty() {
                grouped.push((current_artist, artist_albums));
            }

            for (gi, (artist_name, als)) in grouped.iter().enumerate() {
                if gi > 0 {
                    println!();
                }
                println!("{}", artist_name.bold().cyan());
                for (i, al) in als.iter().enumerate() {
                    let is_last = i == als.len() - 1;
                    let branch = if is_last {
                        "\u{2514}\u{2500}\u{2500} "
                    } else {
                        "\u{251c}\u{2500}\u{2500} "
                    };

                    let year = al
                        .date
                        .as_deref()
                        .map(|d| {
                            let y = if d.len() >= 4 { &d[..4] } else { d };
                            format!("({}) ", y).dimmed().to_string()
                        })
                        .unwrap_or_default();
                    let codec = al
                        .codec
                        .as_deref()
                        .map(|c| format!(" [{}]", c).yellow().dimmed().to_string())
                        .unwrap_or_default();

                    println!(
                        "{}{}{}{} {}",
                        branch.dimmed(),
                        year,
                        al.title.green(),
                        codec,
                        format!("[{}]", al.id).dimmed(),
                    );
                }
            }
        }
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

pub fn cmd_library() {
    let db = open_db();
    match queries::library_stats(&db.conn) {
        Ok(stats) => {
            println!("{}", "library".bold());
            println!(
                "  {} {}",
                "tracks:".cyan(),
                format!("{} total", stats.total_tracks).bold(),
            );
            println!("    {} {}", "local:".dimmed(), stats.local_tracks,);
            println!("    {} {}", "remote:".dimmed(), stats.remote_tracks,);
            println!("    {} {}", "cached:".dimmed(), stats.cached_tracks,);
            println!("  {} {}", "albums:".cyan(), stats.total_albums);
            println!("  {} {}", "artists:".cyan(), stats.total_artists);
            println!(
                "\n{} {}",
                "db:".cyan(),
                config::db_path().display().to_string().dimmed()
            );
        }
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        }
    }
}
