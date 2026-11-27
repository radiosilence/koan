//! The elements a response is made of, named as the Subsonic XSD and the
//! OpenSubsonic additions name them. A name is spelled once, here, and a
//! misspelt one does not compile.

macro_rules! tags {
    ($($variant:ident = $name:literal,)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(in crate::subsonic) enum Tag {
            $($variant,)*
        }

        impl Tag {
            /// The element's name on the wire: the XML element, the JSON member.
            pub(in crate::subsonic) fn as_str(self) -> &'static str {
                match self {
                    $(Tag::$variant => $name,)*
                }
            }
        }
    };
}

tags! {
    Album = "album",
    AlbumArtists = "albumArtists",
    AlbumList = "albumList",
    AlbumList2 = "albumList2",
    ApiKey = "apiKey",
    ApiKeys = "apiKeys",
    AppPassword = "appPassword",
    AppPasswords = "appPasswords",
    Artist = "artist",
    ArtistInfo2 = "artistInfo2",
    Artists = "artists",
    Biography = "biography",
    Bookmark = "bookmark",
    Bookmarks = "bookmarks",
    Child = "child",
    Directory = "directory",
    Dismissed = "dismissed",
    Entry = "entry",
    Error = "error",
    Forgotten = "forgotten",
    Genre = "genre",
    Genres = "genres",
    Index = "index",
    Indexes = "indexes",
    Invite = "invite",
    Join = "join",
    KoanBioUrl = "koanBioUrl",
    KoanCommand = "koanCommand",
    KoanDspDeleted = "koanDspDeleted",
    KoanDspProfiles = "koanDspProfiles",
    KoanDspSaved = "koanDspSaved",
    KoanHistory = "koanHistory",
    KoanImage = "koanImage",
    KoanImageCredit = "koanImageCredit",
    KoanScrobbling = "koanScrobbling",
    LargeImageUrl = "largeImageUrl",
    License = "license",
    Line = "line",
    LyricsList = "lyricsList",
    Mcp = "mcp",
    MediumImageUrl = "mediumImageUrl",
    Missing = "missing",
    MusicBrainzId = "musicBrainzId",
    MusicFolder = "musicFolder",
    MusicFolders = "musicFolders",
    OpenSubsonicExtensions = "openSubsonicExtensions",
    Pair = "pair",
    Play = "play",
    PlayQueue = "playQueue",
    PlayQueueByIndex = "playQueueByIndex",
    Playlist = "playlist",
    Playlists = "playlists",
    Profile = "profile",
    RandomSongs = "randomSongs",
    RecordLabels = "recordLabels",
    ReleaseDate = "releaseDate",
    ReleaseTypes = "releaseTypes",
    ScanStatus = "scanStatus",
    SearchResult3 = "searchResult3",
    Service = "service",
    Share = "share",
    Shares = "shares",
    SmallImageUrl = "smallImageUrl",
    Song = "song",
    Starred2 = "starred2",
    StructuredLyrics = "structuredLyrics",
    SubsonicResponse = "subsonic-response",
    TokenInfo = "tokenInfo",
    User = "user",
    Users = "users",
    Versions = "versions",
}
