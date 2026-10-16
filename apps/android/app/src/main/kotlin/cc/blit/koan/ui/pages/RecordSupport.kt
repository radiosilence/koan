package cc.blit.koan.ui.pages

import androidx.compose.runtime.Immutable
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import cc.blit.koan.state.AlbumRecord
import cc.blit.koan.state.ArtistRecord
import uniffi.koan_ffi.Album
import uniffi.koan_ffi.EntryStatus
import uniffi.koan_ffi.QueueItem
import uniffi.koan_ffi.Track

/*
 * The decisions the album and artist pages make, with no Compose in them: what
 * a record's lines are, where the highlighted track sits, what a track's mark
 * says, what a pick holds, what the biography shows. The pages draw what these
 * return, and these are what the JVM tests check.
 */

// MARK: - Album: the lines of a tracklist

/** One line of a record's list: a disc heading, or a track. */
sealed interface TrackLine {
    /** Stable across reloads: a track by its id, a heading by its disc and place. */
    val key: String

    data class Disc(val disc: Int, val at: Int) : TrackLine {
        override val key: String get() = "disc:$disc:$at"
    }

    /** [position] is the track's place in the whole list, which is where playing it starts. */
    data class Row(val track: Track, val position: Int) : TrackLine {
        override val key: String get() = "track:${track.id}:$position"
    }
}

/**
 * The tracks as lines. A record on several discs gets a heading where each
 * disc begins; one disc, or none given, gets none. The order is the one the
 * engine returned (running order): a heading marks where the disc changes, it
 * does not regroup, so a list that is not sorted by disc is not rearranged.
 */
fun trackLines(tracks: List<Track>): List<TrackLine> {
    val discs = tracks.map { it.disc ?: 1 }
    val headed = discs.toSet().size > 1
    val lines = ArrayList<TrackLine>(tracks.size + if (headed) discs.toSet().size else 0)
    var previous: Int? = null
    tracks.forEachIndexed { index, track ->
        val disc = discs[index]
        if (headed && disc != previous) lines += TrackLine.Disc(disc, lines.size)
        previous = disc
        lines += TrackLine.Row(track, index)
    }
    return lines
}

/**
 * Where in the lazy list a track sits, counting the [leading] items above the
 * lines (the header), or null if this record does not hold it. A highlight for
 * a track on another record is not this page's to consume: the navigator
 * holds one value for every page in every stack.
 */
fun highlightIndex(lines: List<TrackLine>, trackId: Long?, leading: Int = 1): Int? {
    if (trackId == null) return null
    val at = lines.indexOfFirst { it is TrackLine.Row && it.track.id == trackId }
    return if (at < 0) null else at + leading
}

/** What a record page is, given what the library has read. */
sealed interface AlbumView {
    /** Nothing read yet. */
    data object Loading : AlbumView

    /** The read failed; worth trying again. */
    data object Failed : AlbumView

    /** The read found nothing: the record is gone from the library. */
    data object Gone : AlbumView

    data class Ready(val album: Album, val tracks: List<Track>) : AlbumView
}

fun albumView(record: AlbumRecord?): AlbumView = when {
    record == null -> AlbumView.Loading
    record.album != null -> AlbumView.Ready(record.album, record.tracks)
    record.failed -> AlbumView.Failed
    else -> AlbumView.Gone
}

/** "2007 · FLAC · 59:10": what follows the artist in a record's header. */
fun albumDetails(album: Album, tracks: List<Track>): String {
    val parts = ArrayList<String>(3)
    album.year?.let { parts += it.toString() }
    album.codec?.let { parts += it.uppercase() }
    val total = tracks.sumOf { it.durationMs ?: 0L }
    if (total > 0) parts += formatDuration(total)
    return parts.joinToString(" · ")
}

// MARK: - Formatting

/** `m:ss`, or `h:mm:ss` once it earns the hour; `--:--` for a length not known. */
fun formatDuration(ms: Long?): String {
    if (ms == null || ms <= 0) return "--:--"
    val total = (ms / 1000).toInt()
    val (h, m, s) = Triple(total / 3600, (total % 3600) / 60, total % 60)
    return if (h > 0) "%d:%02d:%02d".format(h, m, s) else "%d:%02d".format(m, s)
}

/** "FLAC 24/96", "MP3 44.1 kHz"; null when the codec is not known. */
fun trackQuality(track: Track): String? {
    val codec = track.codec ?: return null
    val rate = track.sampleRate
    val depth = track.bitDepth
    return when {
        depth != null && rate != null -> "${codec.uppercase()} $depth/${kilohertz(rate)}"
        rate != null -> "${codec.uppercase()} ${kilohertz(rate)} kHz"
        else -> codec.uppercase()
    }
}

/** 44100 gives "44.1", 96000 gives "96": a trailing ".0" is noise. */
fun kilohertz(hz: Int): String {
    val khz = hz / 1000.0
    return if (khz == Math.rint(khz)) "%.0f".format(khz) else "%.1f".format(khz)
}

/** "3 albums", "1 album". */
fun countOf(n: Int, singular: String, plural: String = singular + "s"): String = "$n ${if (n == 1) singular else plural}"

/** A dated live-set title loses its date prefix: "2019-04-02: Set" reads "Set". Same rule as the Swift `Format.title`. */
fun displayTitle(raw: String): String {
    val m = DatedTitle.find(raw) ?: return raw
    val rest = raw.substring(m.range.last + 1)
    return rest.ifEmpty { raw }
}

private val DatedTitle = Regex("""^\d{4}-\d{2}-\d{2}\s*[:\-–]\s*""")

// MARK: - A track's mark

/** Where a track's bytes are, in one word; the row draws a glyph for it. */
enum class Availability {
    /** Queued for download behind others. */
    Waiting,

    /** The download is arriving. */
    Downloading,

    /** The download failed. */
    Failed,

    /** On the server and downloaded. */
    Kept,

    /** On the server, comes down on play. */
    Cloud,

    /** A file on this device. */
    Local,

    None,
}

/**
 * What the queue is doing about a track wins over what the library knows,
 * because only the queue knows a download is in flight; with nothing in
 * flight the library's two facts say where the bytes are. A track that is on
 * both server and disk reads as downloaded, which is what it is.
 */
fun availability(track: Track, queued: QueueItem?): Availability = when {
    queued?.status == EntryStatus.PRIORITY_PENDING -> Availability.Waiting
    queued?.status == EntryStatus.DOWNLOADING -> Availability.Downloading
    queued?.status == EntryStatus.FAILED -> Availability.Failed
    track.onServer && track.onDisk -> Availability.Kept
    track.onServer -> Availability.Cloud
    track.onDisk -> Availability.Local
    else -> Availability.None
}

// MARK: - Selection

/**
 * Things picked out of a page to play or queue together: a mode, because a tap
 * already plays or navigates. Held in the order things were ticked, not the
 * order of the page. Ending the mode drops the pick. The port of
 * `PlayableSelection`, as an immutable value the page keeps in its own state.
 */
@Immutable
data class Pick<K>(val active: Boolean = false, val picked: List<K> = emptyList()) {
    private val members: Set<K> get() = picked.toSet()

    val count: Int get() = picked.size

    operator fun contains(key: K): Boolean = key in picked

    /** Enter the mode, optionally with one thing already ticked. */
    fun begin(key: K? = null): Pick<K> = copy(active = true).let { if (key == null) it else it.toggle(key) }

    fun toggle(key: K): Pick<K> =
        if (key in picked) copy(active = true, picked = picked - key) else copy(active = true, picked = picked + key)

    /** Everything the page is showing, appended in page order after what is already ticked. */
    fun selectAll(page: List<K>): Pick<K> {
        val held = members
        return copy(active = true, picked = picked + page.filter { it !in held })
    }

    fun end(): Pick<K> = Pick()
}

// MARK: - Artist

/** What the artist page is, given what the library has read. */
sealed interface ArtistView {
    data object Loading : ArtistView

    /** Read, and nothing there: not an artist the library has. */
    data object Missing : ArtistView

    data class Discography(val albums: List<Album>) : ArtistView

    /** Credited only on other people's records: their tracks, rather than an empty grid. */
    data class Appearances(val tracks: List<Track>, val albumCount: Int) : ArtistView
}

/**
 * The record is one value for whichever artist was read last, so it is
 * guarded on the id: history can move faster than a read, and another
 * artist's page must not draw the wrong person.
 */
fun artistView(record: ArtistRecord?, artistId: Long): ArtistView = when {
    record == null || record.artistId != artistId -> ArtistView.Loading
    record.albums.isNotEmpty() -> ArtistView.Discography(record.albums)
    record.appearances.isNotEmpty() ->
        ArtistView.Appearances(record.appearances, record.appearances.mapNotNull { it.albumId }.toSet().size)
    record.artist == null -> ArtistView.Missing
    else -> ArtistView.Discography(emptyList())
}

/** What the biography area shows. */
sealed interface BioState {
    data object Hidden : BioState

    /** Nothing cached, a network lookup is under way. */
    data object Looking : BioState

    data class Text(val paragraphs: List<String>, val source: String?, val imageCredit: String?) : BioState
}

fun bioState(record: ArtistRecord?): BioState {
    val info = record?.info
    val bio = info?.bio?.takeIf { it.isNotBlank() }
    return when {
        info != null && bio != null ->
            BioState.Text(bio.split('\n').map { it.trim() }.filter { it.isNotEmpty() }, info.bioUrl, info.imageCredit)
        record?.infoLoading == true -> BioState.Looking
        else -> BioState.Hidden
    }
}

/** Whether the artist's photograph is there to draw (the header leaves the space out when it is not). */
fun hasPhoto(record: ArtistRecord?): Boolean = record?.info?.hasImage == true

// MARK: - Grid geometry

private val Gap = 16.dp
private val Page = 22.dp
private val MinTile = 160.dp

/**
 * How many tiles fit across. Two on a phone; more as the width allows, so the
 * header's cover (one column wide) stays a cover and not a poster.
 */
fun gridColumns(width: Dp, page: Dp = Page, gap: Dp = Gap): Int {
    val room = width - page * 2 + gap
    return maxOf(2, (room / (MinTile + gap)).toInt())
}

/** A tile's width in a grid of [columns] across [width]: the width of a record's cover. */
fun tileWidth(width: Dp, columns: Int, page: Dp = Page, gap: Dp = Gap): Dp =
    (width - page * 2 - gap * (columns - 1)) / columns
