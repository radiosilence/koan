//! A large library of tagged, near-silent FLACs with a distinct cover per
//! record, for measuring the browsers against tens of thousands of tracks.
//!
//! cargo run --release -p koan-desktop --example gen_library -- DIR [ALBUMS] [TRACKS_PER_ALBUM]
//!
//! Needs `ffmpeg` for the one seed file; every track is a copy of it, tagged
//! with lofty, so 50 000 tracks take seconds rather than an encode each. The
//! covers are `cover.jpg` beside the tracks, which the scanner prefers to
//! embedded art.

use std::path::{Path, PathBuf};
use std::process::Command;

use image::{ImageBuffer, Rgb};
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag, TagType};

const YEARS: std::ops::Range<u32> = 1960..2026;
const GENRES: [&str; 8] = [
    "Ambient",
    "Jazz",
    "Techno",
    "Folk",
    "Drum & Bass",
    "Rock",
    "Classical",
    "Dub",
];

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(
        args.next()
            .expect("usage: gen_library DIR [ALBUMS] [TRACKS]"),
    );
    let albums: u32 = args.next().map_or(5000, |a| a.parse().expect("ALBUMS"));
    let tracks: u32 = args.next().map_or(10, |a| a.parse().expect("TRACKS"));
    std::fs::create_dir_all(&dir).expect("library directory");

    let seed = dir.join(".seed.flac");
    let made = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg("anullsrc=r=44100:cl=stereo")
        .args(["-t", "0.2", "-c:a", "flac"])
        .arg(&seed)
        .status()
        .expect("ffmpeg");
    assert!(made.success(), "ffmpeg could not make the seed");

    // Five records per artist: a library's shape, and the artist pages get
    // sections worth drawing.
    for a in 0..albums {
        let artist = format!("Artist {:04}", a / 5);
        let album = format!("Record {a:05}");
        let year = YEARS.start + (a * 7919) % YEARS.len() as u32;
        let genre = GENRES[(a as usize) % GENRES.len()];
        let folder = dir.join(&artist).join(&album);
        std::fs::create_dir_all(&folder).expect("record folder");
        cover(a).save(folder.join("cover.jpg")).expect("cover");
        for n in 1..=tracks {
            let file = folder.join(format!("{n:02} Track {n}.flac"));
            std::fs::copy(&seed, &file).expect("copy seed");
            tag(&file, &artist, &album, n, year, genre);
        }
    }
    std::fs::remove_file(seed).ok();
    println!(
        "{} tracks in {albums} records under {}",
        albums * tracks,
        dir.display()
    );
}

fn tag(file: &Path, artist: &str, album: &str, n: u32, year: u32, genre: &str) {
    let mut tag = Tag::new(TagType::VorbisComments);
    tag.set_artist(artist.to_string());
    tag.insert_text(ItemKey::AlbumArtist, artist.to_string());
    tag.set_album(album.to_string());
    tag.set_title(format!("Track {n}"));
    tag.set_track(n);
    tag.set_genre(genre.to_string());
    tag.insert_text(ItemKey::Year, year.to_string());
    tag.save_to_path(file, WriteOptions::default())
        .expect("write tags");
}

/// A 600-pixel sleeve unlike its neighbours: two hues and a band, from the
/// record's number, so a blank or wrong tile is obvious while scrolling.
fn cover(n: u32) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    let h1 = (n.wrapping_mul(2654435761) >> 8) as u8;
    let h2 = (n.wrapping_mul(40503) >> 4) as u8;
    let band = 100 + (n % 400);
    ImageBuffer::from_fn(600, 600, |x, y| {
        let t = (x + y) as f32 / 1200.0;
        let base = if (y as i64 - band as i64).abs() < 40 {
            [240, 240, 240]
        } else {
            [h1, h2, (255.0 * t) as u8]
        };
        Rgb(base)
    })
}
