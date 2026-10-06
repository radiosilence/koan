import KoanFFI
import SwiftUI

/// What koan is fetching from the server, and what it just finished fetching.
///
/// The first place to look when something is not playing: a transfer that has
/// stopped moving says so — its rate falls to nothing while its bar stays put —
/// and a failure keeps its reason rather than only flashing past on the row
/// that caused it.
struct DownloadsView: View {
    @Environment(EngineMirror.self) private var mirror
    @Environment(AppState.self) private var app
    #if os(macOS)
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    @Environment(CoverArtCache.self) private var art
    @Environment(UIState.self) private var ui
    @Environment(TransferMeter.self) private var meter
    @State private var selection: Set<Int64> = []
    #endif

    var body: some View {
        Group {
            if mirror.transfers.isEmpty {
                KoanUnavailable(
                    "Nothing downloading",
                    icon: Icon.downloads,
                    detail: "Tracks fetched from your server appear here while they arrive."
                )
            } else {
                #if os(macOS)
                table
                #else
                List {
                    ForEach(mirror.transfers, id: \.trackId) { transfer in
                        DownloadRow(transfer: transfer)
                            .washedRow()
                    }
                }
                .insetList()
                #endif
            }
        }
        .navigationTitle(KoanTheme.label("Downloads"))
        .toolbar {
            if mirror.hasSettledTransfers {
                Button { app.engine.clearSettledDownloads() } label: {
                    Label("Clear Finished", systemImage: Icon.clear)
                }
                .help("Forget the transfers that have already settled")
                .toolbarButton()
            }
        }
    }
}

#if os(macOS)
extension DownloadsView {
    /// A `KoanTable` — see there for why the Mac's lists are AppKit.
    private var table: some View {
        let transfers = mirror.transfers
        // What the rows draw that changes under them. The figures of the
        // transfers still going are not in it: `TransferMeter` hands those to
        // the rows directly.
        let key = transfers.map { "\($0.trackId):\($0.state)" }
        let library = library
        let nav = nav
        return SafeAreaReader { insets in
            KoanTable(
                items: transfers,
                id: \.trackId,
                context: DownloadTableRow.Context(
                    meter: meter,
                    art: art,
                    showInLibrary: { DownloadMenu.showInLibrary($0, library: library, nav: nav) }
                ),
                contextKey: AnyHashable(key),
                selection: $selection,
                make: DownloadTableRow.init,
                menu: { ids, environment in
                    guard ids.count == 1, let transfer = transfers.first(where: { ids.contains($0.trackId) }) else {
                        return nil
                    }
                    return hostedMenu(DownloadMenu(transfer: transfer), environment: environment)
                },
                primaryAction: { ids in
                    if let transfer = transfers.first(where: { ids.contains($0.trackId) }) {
                        DownloadMenu.showInLibrary(transfer, library: library, nav: nav)
                    }
                },
                selectAllToken: ui.selectAllToken,
                insets: insets
            )
        }
        .clearsSelection($selection)
    }
}
#endif

/// What a download's menu offers, on either platform.
struct DownloadMenu: View {
    let transfer: Transfer

    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library

    var body: some View {
        Button { Self.showInLibrary(transfer, library: library, nav: nav) } label: {
            Label("Show in Library", systemImage: Icon.album)
        }
        if transfer.state == .done {
            Button { library.clearDownloads(trackIds: [transfer.trackId]) } label: {
                Label("Remove Downloaded File", systemImage: Icon.clear)
            }
        }
    }

    /// The record it came off, which is where you go to find it. Resolved when
    /// asked rather than carried on every row — the store holds transfers, not
    /// library rows, and most rows are never clicked.
    static func showInLibrary(_ transfer: Transfer, library: LibraryModel, nav: Navigator) {
        let engine = library.engine
        let trackId = transfer.trackId
        Task {
            guard let albumId = (try? await engine.track(trackId: trackId))??.albumId else {
                return
            }
            nav.open(album: albumId, highlighting: trackId)
        }
    }
}

private struct DownloadRow: View {
    let transfer: Transfer

    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    @Environment(EngineMirror.self) private var mirror
    @Environment(TransferMeter.self) private var meter
    @State private var hovering = false
    @Environment(\.horizontalSizeClass) private var width

    /// The numbers, read here rather than carried on the row. They move ten
    /// times a second while this row is going and not at all once it has
    /// settled — which is exactly what the two slices are for. A settled row
    /// does not read them at all, or it would redraw at the others' rate.
    private var figures: TransferFigure? {
        isRunning ? mirror.figure(for: transfer.trackId) : nil
    }
    private var bytesWritten: UInt64 { figures?.bytesWritten ?? 0 }
    private var totalBytes: UInt64 { figures?.totalBytes ?? 0 }
    private var bytesPerSecond: UInt64 { figures?.bytesPerSecond ?? 0 }
    private var progress: Double? { figures?.progress }

    var body: some View {
        HStack(spacing: 10) {
            // A record is what you recognise a download by, and this is a list
            // of things you are waiting for.
            AlbumArtwork(source: .track(transfer.trackId), size: .thumb, cornerRadius: KoanTheme.radius(3))
                .frame(width: RowMetrics.sleeve, height: RowMetrics.sleeve)

            rows
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
        .pointerHover { hovering = $0 }
        .contextMenu { DownloadMenu(transfer: transfer) }
    }

    private var rows: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(transfer.title)
                    .lineLimit(1)
                Spacer(minLength: 8)
                Text(figure)
                    .font(.role(.fine, system: .caption.monospacedDigit()))
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
            }

            // Drawn rather than a `ProgressView`: the stock linear style
            // rendered the same full-width track whatever it was given, which
            // made six transfers at six different percentages look identical.
            //
            // The quiet end is the part that has not arrived, the same way
            // round as the seek bar. Lighting what *had* arrived meant a
            // transfer finishing took its highlight away and the bar dropped
            // back to looking empty — a row that had just completed read as
            // one that had not started.
            //
            //
            // Layers fed by `TransferMeter`, so the bar moves at the display's
            // rate without this body running for it.
            TransferBar(
                transfer: transfer.state == .running ? transfer.trackId : nil,
                fraction: fraction,
                meter: meter
            )
            .frame(height: 4)

            HStack(spacing: 6) {
                Text(subtitle)
                    .lineLimit(1)
                    .foregroundStyle(
                        transfer.state == .failed ? KoanTheme.style(.bad, system: .orange) : KoanTheme.style(.muted, system: .secondary)
                    )
                Spacer(minLength: 8)
                // No hover on a phone: a link that waits for one never shows.
                if hovering || width == .compact {
                    Button("Show in Library") { showInLibrary() }
                        .linkButton()
                        .font(.role(.fine, system: .caption))
                }
            }
            .font(.role(.fine, system: .caption))
        }
    }

    private func showInLibrary() {
        DownloadMenu.showInLibrary(transfer, library: library, nav: nav)
    }

    private var isRunning: Bool { transfer.state == .running || transfer.state == .queued }

    /// What has arrived.
    private var fraction: Double {
        // Whole once it has landed, whatever the last figure said — the bar
        // never goes backwards on completion.
        if transfer.state == .done { return 1 }
        // A transfer with no stated length has no fraction and draws an empty
        // bar rather than a full one: it is going, not finished.
        return progress ?? 0
    }

    private var subtitle: String {
        switch transfer.state {
        case .failed: transfer.failureReason ?? "Couldn't be fetched"
        case .queued: transfer.artist.isEmpty ? "Waiting" : "\(transfer.artist) — waiting"
        case .done: transfer.artist.isEmpty ? "Downloaded" : "\(transfer.artist) — downloaded"
        case .running: rateAndSize
        }
    }

    /// "Artist · 4.2 MB/s · 210 MB of 451 MB", dropping whatever is not known.
    /// A rate of nothing is a stall, and saying so is the point of the row.
    private var rateAndSize: String {
        var parts: [String] = []
        if !transfer.artist.isEmpty { parts.append(transfer.artist) }
        parts.append(
            bytesPerSecond > 0
                ? "\(Format.bytes(Int64(bytesPerSecond)))/s"
                : "stalled"
        )
        if totalBytes > 0 {
            parts.append(
                "\(Format.bytes(Int64(bytesWritten))) of \(Format.bytes(Int64(totalBytes)))"
            )
        } else if bytesWritten > 0 {
            parts.append(Format.bytes(Int64(bytesWritten)))
        }
        return parts.joined(separator: " · ")
    }

    private var figure: String {
        switch transfer.state {
        case .done: "Done"
        case .failed: "Failed"
        case .queued: "Queued"
        case .running: progress.map { "\(Int($0 * 100))%" } ?? ""
        }
    }
}

/// A download's bar: the whole length quiet, what has arrived lit.
private struct TransferBar: PlatformViewRepresentable {
    /// The transfer to follow while it runs; `nil` holds `fraction`.
    let transfer: Int64?
    let fraction: Double
    let meter: TransferMeter

    typealias PlatformViewType = TransferBarView

    func makeView(context: Context) -> TransferBarView { TransferBarView() }

    func updateView(_ view: TransferBarView, context: Context) {
        view.meter = meter
        view.fraction = fraction
        meter.follow(view, transfer: transfer)
    }

    static func dismantleView(_ view: TransferBarView, coordinator: ()) {
        view.meter?.follow(view, transfer: nil)
    }
}

final class TransferBarView: LayerView, TransferGauge {
    private let track = CALayer()
    private let filled = CALayer()
    weak var meter: TransferMeter?

    var fraction: Double = 0 {
        didSet {
            guard fraction != oldValue else { return }
            layoutLayers()
        }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        for layer in [track, filled] {
            layer.actions = ["bounds": NSNull(), "position": NSNull(), "backgroundColor": NSNull()]
            hostLayer.addSublayer(layer)
        }
        appearanceChanged()
    }

    func take(_ figure: TransferFigure) {
        fraction = figure.progress ?? 0
    }

    override func layoutLayers() {
        let height = bounds.height
        track.cornerRadius = KoanTheme.radius(height / 2)
        filled.cornerRadius = KoanTheme.radius(height / 2)
        track.frame = bounds
        filled.frame = CGRect(x: 0, y: 0, width: bounds.width * fraction.clamped(), height: height)
    }

    override func appearanceChanged() {
        track.backgroundColor = resolved(.quaternaryLabel)
        filled.backgroundColor = resolved(.label)
    }
}
