import KoanFFI
import SwiftUI

/// Sidebar, stage, optional lyrics inspector, transport pinned to the bottom.
///
/// The stage defaults to the queue rather than the library — koan is a player
/// you build a queue in, and the TUI opens the same way. The library is
/// somewhere you go to feed it.
///
/// `NavigationSplitView` is the root and stays the root. Wrapping it in a stack
/// or putting an `HSplitView` in its detail column breaks width propagation:
/// `HSplitView` sizes children to their minimum, so the stage would sit at
/// whatever `minWidth` it declared no matter how large the window got, and an
/// adaptive grid inside it would be stuck at two columns. The transport is a
/// `safeAreaInset` and the lyrics panel an `inspector` for the same reason —
/// both add chrome without taking the detail column's width away.
struct RootView: View {
    @Binding var showingPicker: Bool

    @Environment(LibraryModel.self) private var library
    @Environment(PlayerModel.self) private var player

    @AppStorage("showLyrics") private var showLyrics = false

    var body: some View {
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 190, ideal: 215, max: 290)
        } detail: {
            stage
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .inspector(isPresented: $showLyrics) {
                    LyricsPanel()
                        .inspectorColumnWidth(min: 260, ideal: 320, max: 460)
                }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                Divider()
                TransportBar()
            }
        }
        .toolbar {
            ToolbarItem(placement: .navigation) {
                Button {
                    showingPicker = true
                } label: {
                    Label("Add Music", systemImage: "plus.magnifyingglass")
                }
                .help("Find tracks, albums and artists (⌘K)")
            }
            ToolbarItem(placement: .primaryAction) {
                Button {
                    showLyrics.toggle()
                } label: {
                    Label("Lyrics", systemImage: "text.quote")
                }
                .help("Lyrics panel")
            }
        }
        .sheet(isPresented: $showingPicker) {
            PickerSheet(isPresented: $showingPicker)
        }
        .overlay(alignment: .bottom) {
            if let error = player.lastError {
                ErrorToast(message: error) { player.lastError = nil }
                    .padding(.bottom, 24)
            }
        }
    }

    @ViewBuilder
    private var stage: some View {
        switch library.section {
        case .queue:
            QueueView()
        case .albums:
            AlbumBrowser()
                .searchable(text: filterBinding, placement: .toolbar, prompt: "Filter albums")
        case .artists:
            ArtistBrowser()
                .searchable(text: filterBinding, placement: .toolbar, prompt: "Filter artists")
        case .favourites:
            TrackListView(
                title: "Favourites",
                subtitle: Format.count(Int64(library.visibleFavourites.count), "track"),
                tracks: library.visibleFavourites
            )
            .searchable(text: filterBinding, placement: .toolbar, prompt: "Filter favourites")
        case .snapshots:
            SnapshotsView()
        }
    }

    /// The filter belongs to the library model so it survives switching
    /// sections, but only the browsing views surface a field for it.
    private var filterBinding: Binding<String> {
        Binding(get: { library.filter }, set: { library.filter = $0 })
    }
}

/// Engine errors are informational — a device disappearing shouldn't take a
/// modal to dismiss.
private struct ErrorToast: View {
    let message: String
    let dismiss: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.circle.fill")
                .foregroundStyle(.orange)
            Text(message)
                .font(.callout)
                .lineLimit(2)
            Button(action: dismiss) {
                Image(systemName: "xmark")
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
        .shadow(radius: 12, y: 4)
        .task {
            try? await Task.sleep(for: .seconds(6))
            dismiss()
        }
    }
}
