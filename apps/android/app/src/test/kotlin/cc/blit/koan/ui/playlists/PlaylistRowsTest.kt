package cc.blit.koan.ui.playlists

import cc.blit.koan.ffi.Playlist
import cc.blit.koan.ffi.PlaylistEntry
import cc.blit.koan.ffi.Track
import org.junit.Assert.assertEquals
import org.junit.Test

class PlaylistRowsTest {
    private fun entry(id: Long, album: String, albumId: Long?, artist: String = "Tone Collective") = PlaylistEntry(
        entryId = id,
        track = Track(
            id = id * 10, title = "Track $id", artistName = artist, albumArtistName = artist, albumTitle = album,
            albumId = albumId, artistId = 1, disc = 1, trackNumber = id.toInt(), durationMs = 60_000, codec = "FLAC",
            sampleRate = 44_100, bitDepth = 16, channels = 2, bitrate = null, genre = null, source = "remote",
            onServer = true, onDisk = false, path = null, isFavourite = false,
        ),
    )

    private fun playlist(smart: Boolean = false, readonly: Boolean = false, fromFile: Boolean = false, tracks: UInt = 3u) = Playlist(
        id = 1, name = "Evening", comment = null, remoteId = null, public = false, owner = null, trackCount = tracks,
        durationMs = 0, createdAt = "", changedAt = "", grouped = null, readonly = readonly, smart = smart, fromFile = fromFile,
    )

    @Test
    fun flatIsOneRowPerEntryInOrder() {
        val entries = listOf(entry(1, "A", 1), entry(2, "A", 1), entry(3, "B", 2))
        assertEquals(listOf("entry:1", "entry:2", "entry:3"), playlistRows(entries, grouped = false).map { it.key })
    }

    @Test
    fun groupedHeadsEachRunOfARecord() {
        val entries = listOf(entry(1, "A", 1), entry(2, "A", 1), entry(3, "B", 2), entry(4, "A", 1))
        assertEquals(
            listOf("album:1", "entry:1", "entry:2", "album:3", "entry:3", "album:4", "entry:4"),
            playlistRows(entries, grouped = true).map { it.key },
        )
    }

    @Test
    fun recordsSharingATitleAreToldApartById() {
        val entries = listOf(entry(1, "Greatest Hits", 1), entry(2, "Greatest Hits", 2))
        assertEquals(2, playlistRows(entries, grouped = true).count { it is PlaylistRow.Heading })
    }

    @Test
    fun aTrackWithNoRecordGetsNoHeading() {
        val entries = listOf(entry(1, "", null), entry(2, "", null))
        assertEquals(listOf("entry:1", "entry:2"), playlistRows(entries, grouped = true).map { it.key })
    }

    @Test
    fun summarySaysWhetherAPlaylistIsOneToEdit() {
        assertEquals("3 tracks", playlist().summary())
        assertEquals("1 track · smart", playlist(smart = true, readonly = true, tracks = 1u).summary())
        assertEquals("3 tracks · from a file", playlist(fromFile = true, readonly = true).summary())
        assertEquals("3 tracks · read-only", playlist(readonly = true).summary())
        assertEquals(false, playlist(readonly = true).fillable)
    }
}
