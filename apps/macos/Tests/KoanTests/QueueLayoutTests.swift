@testable import Koan
import KoanFFI
import Testing

/// A queue patch redoes the rows it names and leaves the rest as they were.
@MainActor
struct QueueLayoutTests {
    private func item(_ id: String, run: RunRole, album: Int64? = 1, status: EntryStatus = .queued) -> QueueItem {
        QueueItem(
            queueItemId: id, trackId: 1, albumId: album, artistId: nil, albumArtistId: nil,
            title: id, artist: "", albumArtist: "", artistCredit: [], albumArtistCredit: [],
            creditDiffers: false, run: run, album: "Record", year: nil, codec: nil,
            trackNumber: 1, disc: nil, durationMs: 1000, status: status, playlistEntryId: nil,
            failureReason: nil, onServer: true, onDisk: false
        )
    }

    private var queue: [QueueItem] {
        [
            item("a", run: .opens), item("b", run: .continues), item("c", run: .continues),
            item("d", run: .single, album: 2),
        ]
    }

    @Test func aPatchRedoesOnlyItsRows() {
        let layout = QueueView.Layout()
        var listed = queue
        layout.update(listed, stamp: 1, grouped: true)
        let shape = layout.shape
        let before = layout.lines
        #expect(layout.rows.count == 5)

        listed[1].status = .playing
        #expect(layout.patch([1], from: listed, grouped: true))

        #expect(layout.shape == shape)
        #expect(layout.rows[2].item?.status == .playing)
        if case .album(_, let group) = layout.rows[0] {
            #expect(group.items[1].status == .playing)
        } else {
            Issue.record("the heading moved")
        }
        if case .track(_, let isCurrent, _, _) = layout.lines[2].kind {
            #expect(isCurrent)
        } else {
            Issue.record("the row is not a track")
        }
        for row in [1, 3, 4] { #expect(layout.lines[row] == before[row]) }
    }

    @Test func aPatchThatRegroupsIsLeftToARebuild() {
        let layout = QueueView.Layout()
        var listed = queue
        layout.update(listed, stamp: 1, grouped: true)
        let before = layout.lines

        listed[2].run = .alone
        #expect(!layout.patch([2], from: listed, grouped: true))
        #expect(layout.lines == before)
    }
}
