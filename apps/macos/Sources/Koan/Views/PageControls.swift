#if os(macOS)
import KoanFFI
import SwiftUI

/// What narrows, orders and picks from a page, at the end of its header.
///
/// In the page rather than the window's toolbar: a toolbar item whose content
/// changes as you move between pages makes AppKit re-tile the toolbar, which
/// lays out the whole window on every page switch. A page's header is laid
/// out with the page anyway. iOS asks the same questions its own way — see
/// `RouteView`.
struct PageControls: View {
    var filter: String?
    var sortsAlbums = false
    var selection: PlayableSelection?

    var body: some View {
        HStack(spacing: 8) {
            if let filter {
                LibraryFilter(placeholder: filter)
                    .frame(width: 200)
            }
            if sortsAlbums {
                AlbumSortControls()
            }
            if let selection {
                SelectionControls(selection: selection)
            }
        }
        .controlSize(.regular)
    }
}

/// A page's title, a count under it in all but name, and the page's controls —
/// the header History and Favourites already had, for the pages that had none.
struct PageHeader<Trailing: View>: View {
    let title: String
    let detail: String
    @ViewBuilder let trailing: () -> Trailing

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(title)
                .font(.system(size: 26, weight: .semibold))
            Text(detail)
                .font(.callout)
                .foregroundStyle(.secondary)
            Spacer(minLength: 0)
            trailing()
        }
        .padding(.horizontal, 24)
        .padding(.top, 18)
        .padding(.bottom, 16)
    }
}

/// The filter field, and the only reader of what is typed into it.
///
/// Its own view because the field reads the filter back on every update, and
/// SwiftUI charges that read to whichever body the field sits in. Placed in a
/// page's header directly, every keystroke would re-run the page.
struct LibraryFilter: View {
    let placeholder: String
    @Environment(LibraryModel.self) private var library
    @Environment(UIState.self) private var ui

    var body: some View {
        @Bindable var library = library
        FilterField(placeholder: placeholder, text: $library.filter, focusToken: ui.filterFocusToken)
    }
}

/// Select, or what to do with what has been selected. The only reader of the
/// selection outside the tiles, so a tick re-runs this and not the page.
struct SelectionControls: View {
    let selection: PlayableSelection

    @Environment(LibraryModel.self) private var library
    @Environment(PlayerModel.self) private var player

    var body: some View {
        if selection.isActive {
            let count = selection.picked.count
            HStack(spacing: 2) {
                Button {
                    selection.commit(engine: library.engine, player: player, play: true)
                } label: {
                    Label(count > 0 ? "Play \(count)" : "Play", systemImage: Icon.play)
                        .labelStyle(.titleAndIcon)
                }
                .disabled(count == 0)
                .help("Play the selection, replacing the queue")
                Button {
                    selection.commit(engine: library.engine, player: player, play: false)
                } label: {
                    Label(count > 0 ? "Add \(count) to Queue" : "Add to Queue", systemImage: Icon.queue)
                        .labelStyle(.titleAndIcon)
                }
                .disabled(count == 0)
                .help("Add the selection to the end of the queue")
                Button("Done") { selection.end() }
                    .help("Stop selecting (Esc)")
            }
        } else {
            Button {
                selection.begin()
            } label: {
                Label("Select", systemImage: Icon.selectAll)
            }
            .help("Pick several to play or queue (⌘-click one, or ⌘A)")
        }
    }
}

/// The album grid's sort, and reshuffling when the sort is random.
struct AlbumSortControls: View {
    @Environment(LibraryModel.self) private var library

    var body: some View {
        HStack(spacing: 2) {
            // A pull-down with the current choice ticked, the way Finder's
            // arrange control works — rather than a picker forced to a fixed
            // width, which reads as a control that did not fit.
            Menu {
                Picker("Sort", selection: Binding(
                    get: { library.albumSort },
                    set: { library.albumSort = $0 }
                )) {
                    ForEach(AlbumSort.all, id: \.self) { sort in
                        Text(sort.label).tag(sort)
                    }
                }
                .pickerStyle(.inline)
                .labelsHidden()
            } label: {
                Label("Sort", systemImage: "arrow.up.arrow.down")
            }
            // The accent marks what is playing and what is selected. A
            // control that is always there is neither.
            .tint(.primary)
            .help("Sort albums — \(library.albumSort.label)")

            // Its own button rather than an item inside the sort menu:
            // reshuffling is something you do repeatedly until you like what
            // you see, and a menu makes that four clicks instead of one.
            if library.albumSort == .random {
                Button {
                    library.reshuffleAlbums()
                } label: {
                    Label("Shuffle", systemImage: Icon.reshuffle)
                }
                .tint(.primary)
                .help("Shuffle again")
            }
        }
    }
}
#endif
