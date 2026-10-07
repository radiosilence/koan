#if os(iOS)
import KoanFFI
import SwiftUI

/// Select mode on a phone: what to do with the rows or tiles picked, at the
/// foot of the page, in place of the menu a Mac opens on a multi-selection.
///
/// One bar for every page, so the verbs read the same wherever something can
/// be picked. Each one ends the mode, as playing a pick does on the Mac.
struct SelectionBar: View {
    /// What the page offers for its pick.
    struct Actions {
        /// What favouriting acts on: the things themselves where they are
        /// records or artists, the tracks behind rows otherwise.
        var favourites: [Playable.Key]
        /// The library tracks behind the pick, in the order it was made.
        var tracks: @MainActor () async -> [Int64]
        /// Play, play next and add to queue. The queue's own rows are already
        /// queued, and offer none of them.
        var queues = true
        /// Taking the pick off the page: out of the queue or the playlist, or
        /// forgotten from history.
        var remove: Removal?
    }

    struct Removal {
        let title: String
        let action: @MainActor () -> Void
    }

    let count: Int
    let actions: Actions
    let done: () -> Void

    @Environment(PlayerModel.self) private var player
    @Environment(LibraryModel.self) private var library

    var body: some View {
        VStack(spacing: KoanTheme.Space.xs) {
            HStack {
                Text(count == 0 ? "Select items" : "\(count) selected").koanCase()
                    .koanText(.fine, .muted)
                    .accessibilityAddTraits(.updatesFrequently)
                Spacer()
                Button(action: done) { Text("Done").koanCase() }
                    .koanButton(.standard, system: .borderless)
                    .accessibilityHint("Stops selecting")
            }
            HStack(spacing: 0) {
                if actions.queues {
                    item("Play", short: "Play", icon: Icon.play) { player.playNow(trackIds: $0) }
                    item("Play Next", short: "Next", icon: Icon.playNext) { player.playNext(trackIds: $0) }
                    item("Add to Queue", short: "Queue", icon: Icon.queue) { player.enqueue(trackIds: $0) }
                }
                Menu {
                    AddToPlaylistItems { body in run(body) }
                } label: {
                    BarItem(title: "Add to Playlist", short: "Playlist", icon: Icon.playlist)
                }
                .disabled(count == 0)
                favourite
                if let remove = actions.remove {
                    Button {
                        remove.action()
                        done()
                    } label: {
                        BarItem(title: remove.title, short: remove.title, icon: Icon.remove, destructive: true)
                    }
                    .disabled(count == 0)
                }
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, KoanTheme.Space.l)
        .padding(.top, KoanTheme.Space.s)
        .padding(.bottom, KoanTheme.Space.xs)
        .frame(maxWidth: .infinity)
        .koanRule(.top)
        .koanSurface()
        .background {
            // The platform's look draws its bars over the page; this one sits
            // on a ground of its own as they do.
            if !KoanTheme.isOn { Rectangle().fill(.bar).ignoresSafeArea(edges: .bottom) }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Selection")
    }

    /// Hearts every one of the pick, or, where all of them are hearted
    /// already, takes the hearts off.
    private var favourite: some View {
        let keys = actions.favourites.filter { [.track, .album, .artist].contains($0.kind) }
        let all = !keys.isEmpty && keys.allSatisfy(library.isFavourite)
        return Button {
            for key in keys where library.isFavourite(key) == all {
                library.toggleFavourite(key)
            }
            done()
        } label: {
            BarItem(
                title: all ? "Remove Favourites" : "Favourite",
                short: all ? "Unfavourite" : "Favourite",
                icon: all ? Icon.favourited : Icon.favourite
            )
        }
        .disabled(keys.isEmpty)
    }

    private func item(
        _ title: String, short: String, icon: String, _ body: @escaping @MainActor ([Int64]) -> Void
    ) -> some View {
        Button { run(body) } label: {
            BarItem(title: title, short: short, icon: icon)
        }
        .disabled(count == 0)
    }

    /// Resolves the pick, which for records and artists is a read of the
    /// database, then acts on it and ends the mode.
    private func run(_ body: @escaping @MainActor ([Int64]) -> Void) {
        let tracks = actions.tracks
        done()
        Task {
            let ids = await tracks()
            guard !ids.isEmpty else { return }
            body(ids)
        }
    }
}

/// One verb of the bar: a glyph over a short word, as the tab bar's items are,
/// named in full to VoiceOver.
private struct BarItem: View {
    let title: String
    let short: String
    let icon: String
    var destructive = false

    @Environment(\.koanIcons) private var icons
    @Environment(\.isEnabled) private var enabled

    var body: some View {
        VStack(spacing: KoanTheme.Space.xs) {
            if icons || !KoanTheme.isOn {
                KoanIcon(icon).font(.system(size: 19))
            }
            Text(short).koanCase()
                .font(.role(.fine, system: .caption2))
                .lineLimit(1)
                .minimumScaleFactor(0.75)
        }
        .foregroundStyle(destructive ? KoanTheme.style(.bad, system: .red) : KoanTheme.style(.ink, system: .primary))
        .opacity(enabled ? 1 : 0.4)
        .frame(maxWidth: .infinity, minHeight: 44)
        .contentShape(Rectangle())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
        .accessibilityAddTraits(.isButton)
        .accessibilityShowsLargeContentViewer {
            KoanIcon(icon)
            Text(title)
        }
    }
}

/// "select", where a page's pick begins.
struct SelectButton: View {
    let begin: () -> Void

    var body: some View {
        Button(action: begin) {
            if KoanTheme.isOn {
                Text("Select").koanCase()
            } else {
                Text("Select")
            }
        }
        .koanButton(.text)
        .accessibilityHint("Pick several to play, queue or add to a playlist")
    }
}

extension View {
    /// Select mode over a `List`'s own selection: the List's rows take their
    /// ticks from edit mode, and the bar acts on what is ticked, in the order
    /// it was ticked. `begin` in a toolbar, unless the page has no navigation
    /// bar to put it in and starts the mode itself.
    func listSelectMode<ID: Hashable>(
        _ editMode: Binding<EditMode>,
        selection: Binding<Set<ID>>,
        toolbar: Bool = true,
        actions: @escaping ([ID]) -> SelectionBar.Actions
    ) -> some View {
        modifier(ListSelectMode(editMode: editMode, selection: selection, toolbar: toolbar, actions: actions))
    }

    /// Select mode over a page's `PlayableSelection`: records, artists and
    /// tracks, ticked in the order they are picked, across filters.
    /// `available` is whether the page has anything to pick.
    func playableSelectMode(_ selection: PlayableSelection, engine: KoanEngine, available: Bool) -> some View {
        modifier(PlayableSelectMode(selection: selection, engine: engine, available: available))
    }
}

private struct ListSelectMode<ID: Hashable>: ViewModifier {
    @Binding var editMode: EditMode
    @Binding var selection: Set<ID>
    let toolbar: Bool
    let actions: ([ID]) -> SelectionBar.Actions

    func body(content: Content) -> some View {
        content
            .environment(\.editMode, $editMode)
            .onChange(of: editMode) { _, mode in
                if !mode.isEditing { selection = [] }
            }
            .safeAreaInset(edge: .bottom, spacing: 0) {
                if editMode.isEditing {
                    TickedBar(selection: $selection, actions: actions) { editMode = .inactive }
                }
            }
            .toolbar {
                if toolbar && !editMode.isEditing {
                    ToolbarItem(placement: .topBarTrailing) {
                        SelectButton { editMode = .active }
                    }
                    .sharedBackgroundVisibility(KoanTheme.pane(.automatic))
                }
            }
    }
}

/// The bar over a List's selection, and the one reader of it outside the
/// List, so a tick re-runs this and not the page. Keeps the order things were
/// ticked in, which a set does not.
private struct TickedBar<ID: Hashable>: View {
    @Binding var selection: Set<ID>
    let actions: ([ID]) -> SelectionBar.Actions
    let done: () -> Void
    @State private var order: [ID] = []

    var body: some View {
        SelectionBar(count: order.count, actions: actions(order), done: done)
            .onChange(of: selection, initial: true) { _, now in
                order = order.filter(now.contains) + Array(now.subtracting(order))
            }
    }
}

private struct PlayableSelectMode: ViewModifier {
    let selection: PlayableSelection
    let engine: KoanEngine
    let available: Bool

    func body(content: Content) -> some View {
        content
            .safeAreaInset(edge: .bottom, spacing: 0) {
                if selection.isActive {
                    PickedBar(selection: selection, engine: engine)
                }
            }
            .toolbar {
                if available && !selection.isActive {
                    ToolbarItem(placement: .topBarTrailing) {
                        SelectButton { selection.begin() }
                    }
                    .sharedBackgroundVisibility(KoanTheme.pane(.automatic))
                }
            }
    }
}

/// The bar over a `PlayableSelection`, reading the pick so the page does not.
private struct PickedBar: View {
    let selection: PlayableSelection
    let engine: KoanEngine

    var body: some View {
        let picked = selection.picked
        SelectionBar(
            count: picked.count,
            actions: SelectionBar.Actions(
                favourites: picked.map(\.key),
                tracks: {
                    var tracks: [Int64] = []
                    for item in picked {
                        tracks += await item.trackIds(using: engine)
                    }
                    return tracks
                }
            ),
            done: { selection.end() }
        )
    }
}

extension LibraryModel {
    func isFavourite(_ key: Playable.Key) -> Bool {
        switch key.kind {
        case .track: isFavourite(track: key.id)
        case .album: isFavourite(album: key.id)
        case .artist: isFavourite(artist: key.id)
        case .playlist, .file: false
        }
    }

    func toggleFavourite(_ key: Playable.Key) {
        switch key.kind {
        case .track: toggleFavourite(track: key.id)
        case .album: toggleFavourite(album: key.id)
        case .artist: toggleFavourite(artist: key.id)
        case .playlist, .file: break
        }
    }
}
#endif
