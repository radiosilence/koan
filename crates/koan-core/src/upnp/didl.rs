//! What a renderer is told about a track: DIDL-Lite metadata, and the MIME
//! type it is served as, chosen from what the renderer says it plays.

use super::soap::format_time;
use super::xml::escape;

/// One track, as a renderer is told about it.
pub struct Item<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub url: &'a str,
    pub art_url: Option<&'a str>,
    pub mime: &'a str,
    pub duration_ms: Option<u64>,
    pub size: Option<u64>,
}

/// `DLNA.ORG_OP=01` declares byte-range seeking, without which several
/// renderers refuse to seek at all. The flags say: streaming transfer mode,
/// background transfer mode, connection stalling, DLNA 1.5.
const DLNA_FEATURES: &str =
    "DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000";

pub fn protocol_info(mime: &str) -> String {
    format!("http-get:*:{mime}:{DLNA_FEATURES}")
}

pub fn didl(item: &Item) -> String {
    let mut out = String::from(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" \
xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\" \
xmlns:dlna=\"urn:schemas-dlna-org:metadata-1-0/\">",
    );
    out.push_str(&format!(
        "<item id=\"{}\" parentID=\"0\" restricted=\"1\">",
        escape(item.id)
    ));
    out.push_str(&format!("<dc:title>{}</dc:title>", escape(item.title)));
    if !item.artist.is_empty() {
        out.push_str(&format!(
            "<dc:creator>{0}</dc:creator><upnp:artist>{0}</upnp:artist>",
            escape(item.artist)
        ));
    }
    if !item.album.is_empty() {
        out.push_str(&format!("<upnp:album>{}</upnp:album>", escape(item.album)));
    }
    if let Some(art) = item.art_url {
        out.push_str(&format!(
            "<upnp:albumArtURI>{}</upnp:albumArtURI>",
            escape(art)
        ));
    }
    out.push_str("<upnp:class>object.item.audioItem.musicTrack</upnp:class>");
    out.push_str(&format!(
        "<res protocolInfo=\"{}\"",
        escape(&protocol_info(item.mime))
    ));
    if let Some(ms) = item.duration_ms.filter(|ms| *ms > 0) {
        out.push_str(&format!(
            " duration=\"{}.{:03}\"",
            format_time(ms),
            ms % 1000
        ));
    }
    if let Some(size) = item.size {
        out.push_str(&format!(" size=\"{size}\""));
    }
    out.push_str(&format!(">{}</res></item></DIDL-Lite>", escape(item.url)));
    out
}

/// The MIME types a file may be announced as, most common first. Renderers
/// disagree within each group, and some play only one spelling.
pub fn mime_candidates(extension: &str) -> &'static [&'static str] {
    match extension {
        "flac" => &["audio/flac", "audio/x-flac"],
        "mp3" => &["audio/mpeg", "audio/mp3", "audio/x-mpeg"],
        "m4a" | "m4b" | "mp4" | "alac" | "aac" => {
            &["audio/mp4", "audio/x-m4a", "audio/m4a", "audio/aac"]
        }
        "ogg" | "oga" => &["audio/ogg", "audio/x-ogg", "application/ogg"],
        "opus" => &["audio/opus", "audio/ogg"],
        "wav" => &["audio/wav", "audio/x-wav", "audio/wave"],
        "aif" | "aiff" | "aifc" => &["audio/aiff", "audio/x-aiff"],
        "ape" => &["audio/x-ape", "audio/ape"],
        "wv" => &["audio/x-wavpack", "audio/wavpack"],
        "dsf" => &["audio/x-dsf", "audio/dsf"],
        "dff" => &["audio/x-dff", "audio/dff"],
        _ => &[],
    }
}

/// What a file is, from its first bytes, as the extension it should have.
/// Files are not always named for what they hold: tracks synced before the
/// server published ALAC's suffix were cached as `.bin`.
pub fn sniff_extension(path: &std::path::Path) -> Option<&'static str> {
    use std::io::Read;
    let mut head = [0u8; 40];
    let n = std::fs::File::open(path).ok()?.read(&mut head).ok()?;
    sniff(&head[..n])
}

fn sniff(head: &[u8]) -> Option<&'static str> {
    let at = |offset: usize, magic: &[u8]| head.get(offset..offset + magic.len()) == Some(magic);
    Some(if at(0, b"fLaC") {
        "flac"
    } else if at(0, b"OggS") {
        if head.windows(8).any(|w| w == b"OpusHead") {
            "opus"
        } else {
            "ogg"
        }
    } else if at(0, b"RIFF") && at(8, b"WAVE") {
        "wav"
    } else if at(0, b"FORM") && (at(8, b"AIFF") || at(8, b"AIFC")) {
        "aiff"
    } else if at(4, b"ftyp") {
        "m4a"
    } else if at(0, b"MAC ") {
        "ape"
    } else if at(0, b"wvpk") {
        "wv"
    } else if at(0, b"DSD ") {
        "dsf"
    } else if at(0, b"FRM8") {
        "dff"
    } else if at(0, b"ID3") || (head.len() > 1 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0) {
        "mp3"
    } else {
        return None;
    })
}

/// The MIME types out of a `GetProtocolInfo` sink list. `None` in the list
/// stands for a wildcard, a renderer claiming to play anything.
pub fn sink_mimes(sink: &str) -> Vec<Option<String>> {
    sink.split(',')
        .filter_map(|entry| {
            let mut fields = entry.trim().split(':');
            let protocol = fields.next()?;
            let _network = fields.next()?;
            let mime = fields.next()?;
            (protocol == "http-get" || protocol == "*").then(|| {
                (mime != "*")
                    .then(|| mime.to_ascii_lowercase())
                    .filter(|m| !m.is_empty())
            })
        })
        .collect()
}

/// The MIME type to serve a file with `extension` as, given the renderer's
/// sink list: the first candidate it lists. A renderer that lists nothing, or
/// lists a wildcard, gets the most common spelling. `None` means it cannot
/// play the file.
pub fn choose_mime(extension: &str, sink: &[Option<String>]) -> Option<&'static str> {
    let candidates = mime_candidates(extension);
    if sink.is_empty() || sink.iter().any(Option::is_none) {
        return candidates.first().copied();
    }
    candidates
        .iter()
        .find(|c| sink.iter().flatten().any(|m| m == *c))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn didl_escapes_and_carries_seek_flags() {
        let out = didl(&Item {
            id: "q1",
            title: "Rock & Roll",
            artist: "A<B",
            album: "",
            url: "http://h:1/t/abc.flac?x=1&y=2",
            art_url: Some("http://h:1/art/abc"),
            mime: "audio/flac",
            duration_ms: Some(192_250),
            size: Some(1234),
        });
        assert!(out.contains("<dc:title>Rock &amp; Roll</dc:title>"));
        assert!(out.contains("<upnp:artist>A&lt;B</upnp:artist>"));
        assert!(!out.contains("upnp:album>"));
        assert!(out.contains("protocolInfo=\"http-get:*:audio/flac:DLNA.ORG_OP=01;"));
        assert!(out.contains("duration=\"0:03:12.250\""));
        assert!(out.contains("size=\"1234\""));
        assert!(out.contains(">http://h:1/t/abc.flac?x=1&amp;y=2</res>"));
        assert!(super::super::xml::parse(&out).is_ok());
    }

    #[test]
    fn the_renderers_own_spelling_is_chosen() {
        let sink = sink_mimes(
            "http-get:*:audio/x-flac:*,http-get:*:audio/mpeg:DLNA.ORG_PN=MP3,rtsp-rtp-udp:*:audio/L16:*",
        );
        assert_eq!(choose_mime("flac", &sink), Some("audio/x-flac"));
        assert_eq!(choose_mime("mp3", &sink), Some("audio/mpeg"));
        assert_eq!(choose_mime("opus", &sink), None);
        assert_eq!(choose_mime("xyz", &sink), None);
    }

    #[test]
    fn a_file_is_known_by_its_first_bytes() {
        assert_eq!(sniff(b"\0\0\0\x1cftypM4A \0\0"), Some("m4a"));
        assert_eq!(sniff(b"fLaC\0\0\0\x22"), Some("flac"));
        assert_eq!(sniff(b"ID3\x04\0"), Some("mp3"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), Some("wav"));
        let mut opus = b"OggS".to_vec();
        opus.resize(28, 0);
        opus.extend_from_slice(b"OpusHead");
        assert_eq!(sniff(&opus), Some("opus"));
        assert_eq!(sniff(b"hello"), None);
    }

    #[test]
    fn a_wildcard_or_empty_sink_takes_the_common_spelling() {
        assert_eq!(
            choose_mime("flac", &sink_mimes("http-get:*:*:*")),
            Some("audio/flac")
        );
        assert_eq!(choose_mime("m4a", &[]), Some("audio/mp4"));
    }
}
