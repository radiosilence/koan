import KoanFFI
import SwiftUI

/// The queue is the main stage, the way it is in the TUI — the library is
/// somewhere you visit to feed this, not the other way round.
///
/// Tracks are grouped under album headers, and the grouping follows *contiguous
/// runs* rather than sorting: queue order is the user's, and collapsing two
/// separate visits to the same record into one heading would misrepresent it.
struct QueueView: View {
    #if os(macOS)
    private static let emptyDetail = "Press ⌘K to find something to play."
    #else
    /// Names the pages as their tabs and rows show them.
    private static var emptyDetail: String {
        KoanTheme.isOn
            ? "Find something to play from albums or artists."
            : "Find something to play from Albums or Artists."
    }
    #endif

    /// A phone's library is a server's, so an empty one means not signed in
    /// yet: the first thing anyone opening the app sees, App Review included.
    private var emptyDetail: String {
        if mirror.signInRefused {
            return EngineMirror.signInRefusedDetail
        }
        #if os(iOS) || os(tvOS)
        if library.stats?.totalTracks == 0 {
            return library.emptyLibraryDetail
        }
        #endif
        return Self.emptyDetail
    }

    @Environment(PlayerModel.self) private var player
    @Environment(EngineMirror.self) private var mirror
    @Environment(Navigator.self) private var nav
    @Environment(LibraryModel.self) private var library
    @Environment(OrganizeModel.self) private var organize
    #if os(macOS)
    @Environment(\.openWindow) private var openWindow
    #endif
    @Environment(UIState.self) private var ui
    @Environment(PlaylistsModel.self) private var playlists
    /// The queue outlives the page you are on — see `StageView`. Anything
    /// aimed at whatever list is in front of you has to check.
    @Environment(\.onStage) private var onStage
    @Environment(\.roomTint) private var roomTint
    #if os(macOS)
    @Environment(CoverArtCache.self) private var art
    @Environment(PlayingLevels.self) private var levels
    @Environment(TransferMeter.self) private var meter
    @Environment(\.roomTint) private var tint
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    #endif

    /// Grouped or one row per track. Persisted because it is a preference about
    /// how you listen rather than about the queue in front of you: an album
    /// listen wants the headings, a long shuffled queue wants every row to say
    /// what it is and show its own sleeve.
    @AppStorage("queueGrouped") private var grouped = true

    /// Selection is local `@State`, and this body never reads it.
    ///
    /// Reading an observable in the body means every selection change
    /// invalidates the whole view and rebuilds the List — under the very click
    /// that caused it. A `@State` read here is the same invalidation with a
    /// different owner, so nothing here reads it: the
    /// rows learn they are selected from the List's own `backgroundProminence`,
    /// and the header's count and the mirror to the model live in
    /// `QueueSelectionHeader`, which holds the binding and re-runs alone.
    @State private var selection: Set<String> = []

    #if os(iOS)
    /// A List with a selection binding shows its selection circles and drag
    /// handles unless told otherwise, which on a phone is a queue permanently
    /// in the middle of being edited. Selecting is asked for, from the menu.
    @State private var editMode: EditMode = .inactive
    #endif

    /// Album headings are rows in their own right, not decoration attached to
    /// the first track. That is what lets an album be selected and dragged as a
    /// unit — and stops selecting a track from lighting up the heading above it.
    private var rows: [Row] {
        grouped ? Row.build(from: listed) : listed.map(Row.track)
    }

    /// What the list is laid out from. On the Mac, the queue as it reads now:
    /// the table compares its rows and redraws only those on screen. Elsewhere
    /// the rows as last sent whole, which a track change or a download does not
    /// move, with each row drawn reading its own state — on a queue of tens of
    /// thousands, regrouping and diffing the whole list for each was what made
    /// the phone unusable.
    private var listed: [QueueItem] {
        #if os(macOS)
        player.queue
        #else
        mirror.queueRows
        #endif
    }

    var body: some View {
        // Once per pass. Built again for the header's count and again for the
        // rows, a large queue would be grouped twice per evaluation.
        let rows = self.rows

        VStack(spacing: 0) {
            header(rows)

            if listed.isEmpty {
                EmptyState(
                    icon: "list.bullet",
                    title: "Queue is empty",
                    detail: emptyDetail
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                #if os(iOS) || os(tvOS)
                .task { if library.stats == nil { library.loadStats() } }
                #endif
            } else {
                #if os(macOS)
                table(rows)
                #else
                list(rows)
                #endif
            }
        }
        // On the whole stage, not the List: an empty queue is exactly when you
        // want to drop a folder on it, and it has no rows to land on.
        .dropTarget(for: URL.self) { urls, _ in
            player.importFiles(urls)
            return true
        }
    }

    #if os(macOS)
    /// A `KoanTable` — see there for why the Mac's lists are AppKit.
    private func table(_ rows: [Row]) -> some View {
        let lines = rows.map(line)
        let live = onStage && !reduceMotion
        let offline = mirror.connection?.offline == true
        let jumpTarget: String? = switch ui.queueJumpTarget {
        case .top: rows.first?.id
        case .bottom: rows.last?.id
        case .playing: player.currentItemId
        }
        let jumpPlace: JumpPlace = switch ui.queueJumpTarget {
        case .top: .top
        case .bottom: .bottom
        case .playing: .centre
        }
        let key: [AnyHashable] = [
            AnyHashable(player.isPlaying), AnyHashable(live), AnyHashable(onStage), AnyHashable(tint),
            AnyHashable(library.favouriteTrackIds), AnyHashable(offline),
        ]
        return SafeAreaReader { insets in
            KoanTable(
                items: lines,
                id: \.id,
                context: QueueTableRow.Context(
                    isPlaying: player.isPlaying,
                    barsLive: live,
                    tint: NSColor(tint),
                    favourites: library.favouriteTrackIds,
                    meter: meter,
                    onStage: onStage,
                    art: art,
                    levels: levels,
                    toggleFavourite: { library.toggleFavourite(track: $0) },
                    offline: offline
                ),
                contextKey: AnyHashable(key),
                selection: $selection,
                make: QueueTableRow.init,
                heightOf: QueueTableRow.height(of:),
                changed: { $0 != $1 },
                menu: { ids, environment in hostedMenu(menu(forRows: ids), environment: environment) },
                primaryAction: { play(rowIds: $0) },
                delete: { _ in removeSelected() },
                selectAllToken: onStage ? ui.selectAllToken : 0,
                move: { ids, before in
                    let moving = IndexSet(ids.compactMap { id in rows.firstIndex { $0.id == id } })
                    move(from: moving, to: before)
                },
                jump: (ui.queueJumpToken, jumpTarget, jumpPlace),
                follow: ui.followingQueue ? player.currentItemId : nil,
                userScrolled: { if ui.followingQueue { ui.followingQueue = false } },
                insets: EdgeInsets(top: 0, leading: insets.leading, bottom: insets.bottom, trailing: 0)
            )
        }
        .clearsSelection($selection)
    }

    /// A row of the queue as the table draws it.
    private func line(_ row: Row) -> QueueLine {
        switch row {
        case .album(let id, let group):
            QueueLine(id: id, kind: .heading(QueueHeading(
                title: group.title,
                artist: group.album.isEmpty ? nil : (group.albumArtist.isEmpty ? "Unknown Artist" : group.albumArtist),
                detail: group.detail,
                sleeve: group.items.first?.sleeve,
                sleeveSize: 52
            )))
        case .single(let item):
            QueueLine(id: item.queueItemId, kind: .track(
                QueueRowContent(item: item), isCurrent: item.status == .playing, showArtist: true, artwork: true
            ))
        case .track(let item):
            QueueLine(id: item.queueItemId, kind: .track(
                QueueRowContent(item: item),
                isCurrent: item.status == .playing,
                showArtist: !grouped || item.artist != item.albumArtist,
                artwork: !grouped
            ))
        }
    }
    #endif

    private func list(_ rows: [Row]) -> some View {
                ScrollViewReader { scroll in
                    List(selection: $selection) {
                        ForEach(rows) { row in
                            rowView(row)
                        }
                        .onMove(perform: move)
                    }
                    .insetList()
                    #if os(iOS)
                    // Started from the header: the queue has no navigation bar.
                    .listSelectMode($editMode, selection: $selection, toolbar: false) { ids in
                        let items = Row.itemIds(in: Set(ids), of: rows)
                        let wanted = Set(items)
                        let tracks = mirror.queue.filter { wanted.contains($0.queueItemId) }.compactMap(\.trackId)
                        return SelectionBar.Actions(
                            favourites: tracks.map { Playable.Key(kind: .track, id: $0) },
                            tracks: { tracks },
                            queues: false,
                            remove: .init(title: "Remove") { player.remove(itemIds: items) }
                        )
                    }
                    #endif
                    .washedGround()
                    // `g` / `G`. Watches the token rather than the edge: jumping
                    // to where you already are still has to scroll, because the
                    // list may have been moved since.
                    .onChange(of: ui.queueJumpToken) { _, _ in
                        jump(to: ui.queueJumpTarget, using: scroll)
                    }
                    // Following: the playing track kept in view as it moves
                    // on, until the person scrolls. A view of its own, so
                    // what is playing is never read by this body.
                    .background { FollowPlaying(scroll: scroll) }
                    .onScrollPhaseChange { _, phase in
                        if phase == .interacting, ui.followingQueue {
                            ui.followingQueue = false
                        }
                    }
                    // Double-click and context menu both come from the List, keyed
                    // on the rows under the pointer rather than on a gesture.
                    .selectionMenu(for: String.self) { ids in
                        menu(forRows: ids)
                    } primaryAction: { ids in
                        play(rowIds: ids)
                    }
                    // Enter plays the selection, the way Return opens things
                    // everywhere else on the platform.
                    .onKeyPress(.return) {
                        play(rowIds: selection)
                        return .handled
                    }
                    #if os(macOS)
                    .onDeleteCommand { removeSelected() }
                    .onChange(of: ui.selectAllToken) { _, _ in
                        guard onStage else { return }
                        selection = Set(rows.map(\.id))
                    }
                    .clearsSelection($selection)
                    #endif
                }
    }

    // MARK: - Header

    private func header(_ rows: [Row]) -> some View {
        // A queue that is one record, grouped, has that record's heading as its
        // first row, sleeve and counts and all. The theme says it once.
        let albumOnce = if KoanTheme.isOn, grouped, case .album = mirror.lock { true } else { false }
        // On a phone each control is its own 44-point target, which spaces
        // them already.
        return HStack(spacing: Self.headerSpacing) {
            // What the queue *is*, when it is still something. A queue that
            // came from a playlist and has not been touched since follows that
            // playlist, and saying so is what makes the following legible: you
            // can see why an edit over there moved something here, and you can
            // see the moment it stops.
            HStack(spacing: 12) {
            switch albumOnce ? nil : mirror.lock {
            case .playlist(let playlist):
                PlaylistArtwork(
                    sources: playlists.covers[playlist.id] ?? [],
                    cornerRadius: KoanTheme.radius(4)
                )
                .frame(width: 34, height: 34)
                .koanShadow(0.25, radius: 3, y: 1)
            case .album(let album):
                AlbumArtwork(source: .album(album.id), size: .thumb, cornerRadius: KoanTheme.radius(4))
                    .frame(width: 34, height: 34)
                    .koanShadow(0.25, radius: 3, y: 1)
            case nil:
                EmptyView()
            }

            VStack(alignment: .leading, spacing: 1) {
                if let name = lockedName, !albumOnce {
                    // On a phone the label leaves the name a few letters; the
                    // sleeve beside it already says the queue follows it.
                    #if os(iOS)
                    Text(Format.title(name))
                        .font(.role(.body, system: .headline))
                        .lineLimit(1)
                    #else
                    Text("\(KoanTheme.label("Playing")) \(name)")
                        .font(.role(.body, system: .headline))
                        .lineLimit(1)
                    #endif
                } else if !KoanTheme.tabRootTitle("Queue").isEmpty {
                    Text("Queue").koanCase()
                        .font(.role(.body, system: .headline))
                }
                if !albumOnce {
                    QueueSummary()
                        .font(.role(.fine, system: .caption))
                        .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                        .lineLimit(1)
                }
            }
            }

            Spacer(minLength: 12)

            #if os(iOS)
            // The bar at the foot says what is picked and what to do with it.
            if !editMode.isEditing {
                SelectButton { editMode = .active }
                    .disabled(listed.isEmpty)
                    .fixedSize()
            }
            #else
            QueueSelectionHeader(selection: $selection, rows: rows) { removeSelected() }
            #endif

            JumpToPlayingButton()

            // Both modes shown with the active one lit, the way Finder switches
            // view. A single icon has to choose between naming the mode you are
            // in and the mode you would get, and whichever it picks the other
            // reading is available and wrong.
            if KoanTheme.isOn {
                // The theme's segmented control: the options bare, the chosen
                // one lit, no track.
                HStack(spacing: Self.headerSpacing == 0 ? 0 : KoanTheme.Space.s) {
                    layoutOption(true, Icon.album, "Group by album")
                    layoutOption(false, Icon.queueSection, "One row per track")
                }
            } else {
                Picker("Queue layout", selection: $grouped) {
                    Image(systemName: Icon.album).tag(true)
                    Image(systemName: Icon.queueSection).tag(false)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                .help("Group by album, or one row per track")
            }

            // Undo is a keyboard's idea of a control. The buttons exist to show
            // ⌘Z is available, and there is no ⌘Z on a phone.
            #if os(macOS)
            Button { player.undo() } label: { Image(systemName: Icon.undo) }
                .help("Undo (⌘Z)")
            Button { player.redo() } label: { Image(systemName: Icon.redo) }
                .help("Redo (⇧⌘Z)")
            #endif

            Menu {
                // Playlists are made elsewhere; a television plays them.
                #if !os(tvOS)
                Button {
                    playlists.naming = player.queue.compactMap(\.trackId)
                } label: {
                    Label("Save as Playlist…", systemImage: Icon.playlist)
                }
                Divider()
                #endif
                Button(role: .destructive) { player.clearQueue() } label: {
                    Label("Clear Queue", systemImage: Icon.clear)
                }
            } label: {
                #if os(tvOS)
                Image(systemName: "ellipsis")
                #else
                Image(systemName: KoanTheme.isOn ? "ellipsis" : "ellipsis.circle")
                #endif
            }
            #if os(tvOS)
            .accessibilityLabel("More")
            #else
            // A secondary control, in ink as its neighbours are; a menu's label
            // otherwise takes the accent. The system look keeps the record's.
            .tint(KoanTheme.isOn ? Color.koanInk : roomTint)
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            #if os(iOS)
            .touchTarget()
            #else
            .frame(width: 22)
            #endif
            #endif
        }
        // A television's controls are the size of its other buttons: a
        // borderless glyph is too small to find from across the room.
        #if os(tvOS)
        .buttonStyle(TelevisionButton())
        #else
        .buttonStyle(.borderless)
        #endif
        // On the rows' edges: an inset list keeps 20 clear on a phone.
        #if os(iOS)
        .padding(.horizontal, 20)
        #else
        .padding(.horizontal, 16)
        #endif
        .padding(.vertical, 11)
    }

    /// One option of the theme's segmented control, as glyphs: the chosen one
    /// in `ink` over an accent underline, the other `muted`.
    private func layoutOption(_ value: Bool, _ icon: String, _ label: String) -> some View {
        Button { grouped = value } label: {
            Image(systemName: icon)
                .padding(.bottom, KoanTheme.Space.xs)
                .overlay(alignment: .bottom) {
                    if grouped == value { Rectangle().fill(.tint).frame(height: KoanTheme.hairline) }
                }
                .touchTarget()
        }
            .foregroundStyle(KoanTheme.style(grouped == value ? .ink : .muted))
            .accessibilityLabel(label)
            .accessibilityAddTraits(grouped == value ? .isSelected : [])
            .help(label)
    }

    #if os(iOS)
    private static let headerSpacing: CGFloat = 0
    #else
    private static let headerSpacing: CGFloat = 12
    #endif

    /// What the queue is, when it is still something someone chose.
    private var lockedName: String? {
        switch mirror.lock {
        case .playlist(let playlist): playlist.name
        case .album(let album): album.title
        case nil: nil
        }
    }

    /// Extracted because the type checker gives up on a switch this size
    /// inline in a ForEach.
    @ViewBuilder
    private func rowView(_ row: Row) -> some View {
        switch row {
        case .album(_, let group):
            QueueAlbumHeader(group: group)
                // No playable: a queue album is a run of queue items, not a
                // library album, so its actions are its own.
                .rowBehaviour()
        // A record that is only this track: one row, with its own sleeve and
        // artist, rather than a heading and a row repeating it.
        case .single(let item):
            LiveQueueRow(sent: item, showArtist: true, artwork: true)
            .rowBehaviour()
            // Built from the row in hand: on tvOS the menu is made with the row,
            // and going through `rows` would regroup the whole queue for each.
            .primaryTap { play(rowIds: [item.queueItemId]) } menu: { trackMenu(mirror.queueItem(item.queueItemId) ?? item) }
        case .track(let item):
            LiveQueueRow(
                sent: item,
                // Ungrouped there is no heading above to say what record this
                // is, so the row says it itself.
                showArtist: !grouped || item.artist != item.albumArtist,
                artwork: !grouped
            )
            .rowBehaviour()
            // Built from the row in hand: on tvOS the menu is made with the row,
            // and going through `rows` would regroup the whole queue for each.
            .primaryTap { play(rowIds: [item.queueItemId]) } menu: { trackMenu(mirror.queueItem(item.queueItemId) ?? item) }
        }
    }

    @ViewBuilder
    private func albumMenu(_ group: QueueGroup) -> some View {
        Button {
            if let first = group.items.first { player.play(itemId: first.queueItemId) }
        } label: {
            Label("Play", systemImage: Icon.play)
        }
        Button {
            player.remove(itemIds: group.items.map(\.queueItemId))
        } label: {
            Label("Remove Album", systemImage: Icon.remove)
        }
        Divider()
        AddToPlaylistMenu { $0(group.items.compactMap(\.trackId)) }
        Divider()
        organizeButton(trackIds: group.items.compactMap(\.trackId), title: group.album)
        if let trackId = group.items.compactMap(\.trackId).first {
            Button { showInLibrary(trackId: trackId, highlight: false) } label: {
                Label("Go to Album", systemImage: Icon.album)
            }
        }
        Button {
            Share.link(
                trackIds: group.items.compactMap(\.trackId),
                named: "\(group.albumArtist) — \(group.album)",
                engine: library.engine,
                player: player
            )
        } label: {
            #if os(tvOS)
            Label("Share Album…", systemImage: Icon.share)
            #else
            Label("Copy Album Share Link", systemImage: Icon.share)
            #endif
        }
    }

    /// Find the album a queue item came from, then go there.
    ///
    /// Resolved when the button is pressed, not while the menu is built:
    /// SwiftUI builds context menus as it builds rows, so a lookup there would
    /// run a blocking query per row and freeze the window on a large queue.
    private func showInLibrary(trackId: Int64, highlight: Bool) {
        let engine = library.engine
        Task {
            let albumId = (try? await engine.track(trackId: trackId))??.albumId
            guard let albumId else {
                player.report("That track is no longer in the library.")
                return
            }
            nav.open(album: albumId, highlighting: highlight ? trackId : nil)
        }
    }

    @ViewBuilder
    private func trackMenu(_ item: QueueItem) -> some View {
        Button { player.play(itemId: item.queueItemId) } label: {
            Label("Play", systemImage: Icon.play)
        }
        Button { player.remove(itemIds: [item.queueItemId]) } label: {
            Label("Remove", systemImage: Icon.remove)
        }
        if let trackId = item.trackId {
            Divider()
            AddToPlaylistMenu { $0([trackId]) }
            Divider()
            organizeButton(trackIds: [trackId], title: item.title)
            let favourited = library.isFavourite(track: trackId)
            Button { library.toggleFavourite(track: trackId) } label: {
                Label(
                    favourited ? "Remove Favourite" : "Favourite Track",
                    systemImage: favourited ? Icon.favourited : Icon.favourite
                )
            }
            Button { showInLibrary(trackId: trackId, highlight: true) } label: {
                Label("Go to Album", systemImage: Icon.album)
            }
            if item.onDisk {
                Button { library.clearDownloads(trackIds: [trackId]) } label: {
                    Label("Remove Downloaded File", systemImage: Icon.clear)
                }
            } else {
                Button { library.downloadToCache(trackIds: [trackId]) } label: {
                    Label("Download to Cache", systemImage: Icon.downloads)
                }
            }
            Button {
                Share.link(
                    trackIds: [trackId],
                    named: item.title,
                    engine: library.engine,
                    player: player
                )
            } label: {
                Label(Share.label, systemImage: Icon.share)
            }
        }
    }

    // MARK: - Selection

    /// Queue items behind the selection. Selecting an album heading means its
    /// whole run, which is the point of the heading being selectable.
    private var selectedItemIds: [String] { itemIds(in: selection) }

    /// Expand a set of row ids to the queue items they stand for. An album
    /// heading stands for its whole run; a track stands for itself.
    private func itemIds(in rowIds: Set<String>) -> [String] {
        Row.itemIds(in: rowIds, of: rows)
    }

    private func removeSelected() {
        player.remove(itemIds: selectedItemIds)
        selection = []
    }

    /// Play the first queue item the given rows stand for — the track itself,
    /// or the first track of the album whose heading was double-clicked.
    private func play(rowIds: Set<String>) {
        guard let id = itemIds(in: rowIds).first else { return }
        player.play(itemId: id)
    }


    /// Scrolls only. The TUI's `g` moves a cursor because the cursor is how you
    /// look around there; here the pointer and the selection are separate things
    /// and moving the selection would throw away what you had picked.
    ///
    /// The playing row is centred rather than put at the top: what is playing
    /// is read against what comes after it, and a row at the top edge has no
    /// after.

    private func jump(to target: UIState.Jump, using scroll: ScrollViewProxy) {
        let row: String? = switch target {
        case .top: rows.first?.id
        case .bottom: rows.last?.id
        // A track row's id *is* its queue item's, in either layout.
        case .playing: player.currentItemId
        }
        guard let row else { return }
        let anchor: UnitPoint = switch target {
        case .top: .top
        case .bottom: .bottom
        case .playing: .center
        }
        withAnimation(.easeOut(duration: 0.18)) {
            scroll.scrollTo(row, anchor: anchor)
        }
    }

    /// The menu for whatever is under the pointer. An album heading gets the
    /// album's actions; anything else gets the track's.
    @ViewBuilder
    private func menu(forRows ids: Set<String>) -> some View {
        if ids.count == 1, let row = rows.first(where: { ids.contains($0.id) }) {
            switch row {
            case .album(_, let group): albumMenu(group)
            // As it reads now: whether it is on disk moves without an edit.
            case .track(let item), .single(let item): trackMenu(mirror.queueItem(item.queueItemId) ?? item)
            }
        } else {
            Button { player.remove(itemIds: itemIds(in: ids)) } label: {
                Label("Remove", systemImage: Icon.remove)
            }
            Divider()
            AddToPlaylistMenu { $0(trackIds(in: ids)) }
            Divider()
            organizeButton(trackIds: trackIds(in: ids), title: nil)
            Button { library.downloadToCache(trackIds: trackIds(in: ids)) } label: {
                Label("Download to Cache", systemImage: Icon.downloads)
            }
            Button { library.clearDownloads(trackIds: trackIds(in: ids)) } label: {
                Label("Remove Downloaded Files", systemImage: Icon.clear)
            }
        }
    }

    /// Library track IDs behind a set of rows. A queue item with no row — a
    /// file whose import failed — has no metadata to build a path from.
    private func trackIds(in rowIds: Set<String>) -> [Int64] {
        let wanted = Set(itemIds(in: rowIds))
        return player.queue.filter { wanted.contains($0.queueItemId) }.compactMap(\.trackId)
    }

    /// `title` names the one thing being organized; a multi-selection has no
    /// name, so it is described by its size instead.
    @ViewBuilder
    private func organizeButton(trackIds: [Int64], title: String?) -> some View {
        // Renames files on disk; a phone has no library folder, and no
        // Organize window to open.
        #if os(macOS)
        Button {
            // The window opens first: `begin` reads the config and resolves the
            // selection, and waiting on that would leave the click dead.
            openWindow(id: OrganizeWindow.id)
            Task {
                await organize.begin(
                    title: title ?? Format.count(Int64(trackIds.count), "track"),
                    trackIds: trackIds
                )
            }
        } label: {
            Label("Organize Files…", systemImage: Icon.organize)
        }
        .disabled(trackIds.isEmpty)
        #endif
    }

    // MARK: - Reordering

    /// Moving a heading moves its whole album, which is why headings are rows.
    ///
    /// Anchors to the row being dropped *onto* and inserts before it, rather
    /// than to the row above and inserting after. The latter has no way to
    /// express "at the very top", and lands a drop on an album heading below
    /// that album's first track instead of above the album.
    private func move(from source: IndexSet, to destination: Int) {
        // Ordered, not a Set: these keep their relative order in the queue, and
        // dragging an album must not scramble its tracks.
        let moving = source.sorted().flatMap { rows[$0].itemIds }
        let movingSet = Set(moving)
        guard !moving.isEmpty else { return }

        // The first row at or after the drop that isn't itself being moved.
        if let target = rows[min(destination, rows.count)...]
            .first(where: { row in !row.itemIds.contains(where: movingSet.contains) })?
            .itemIds.first
        {
            player.move(itemIds: moving, target: target, after: false)
            return
        }

        // Nothing below the drop stays put: this is a move to the end.
        guard let last = player.queue.last(where: { !movingSet.contains($0.queueItemId) }) else {
            return
        }
        player.move(itemIds: moving, target: last.queueItemId, after: true)
    }
}

// MARK: - Rows

extension QueueView {
    /// A queue row: either an album heading or one track.
    enum Row: Identifiable {
        case album(id: String, group: QueueGroup)
        case track(QueueItem)
        /// A track that is its whole record, folded into one row.
        case single(QueueItem)

        var id: String {
            switch self {
            case .album(let id, _): id
            case .track(let item), .single(let item): item.queueItemId
            }
        }

        /// The queue items this row stands for.
        var itemIds: [String] {
            switch self {
            case .album(_, let group): group.items.map(\.queueItemId)
            case .track(let item), .single(let item): [item.queueItemId]
            }
        }

        /// Expand a set of row ids to the queue items they stand for.
        static func itemIds(in rowIds: Set<String>, of rows: [Row]) -> [String] {
            rows.filter { rowIds.contains($0.id) }.flatMap(\.itemIds)
        }

        /// Contiguous runs, mirroring the TUI: queue order is the user's, and
        /// collapsing two separate visits to the same record into one heading
        /// would misrepresent it. A heading precedes each run; tracks with no
        /// album stand alone.
        static func build(from queue: [QueueItem]) -> [Row] {
            var rows: [Row] = []
            var index = 0
            while index < queue.count {
                let first = queue[index]
                guard !first.album.isEmpty else {
                    rows.append(.track(first))
                    index += 1
                    continue
                }
                let run = queue[index...].prefix {
                    $0.album == first.album && $0.albumArtist == first.albumArtist
                }
                if run.count == 1,
                   first.title.trimmingCharacters(in: .whitespaces)
                       .caseInsensitiveCompare(first.album.trimmingCharacters(in: .whitespaces))
                       == .orderedSame {
                    rows.append(.single(first))
                    index += 1
                    continue
                }
                rows.append(.album(
                    id: "album:\(first.queueItemId)",
                    group: QueueGroup(
                        id: first.queueItemId,
                        albumArtist: first.albumArtist,
                        album: first.album,
                        items: Array(run)
                    )
                ))
                rows.append(contentsOf: run.map { Row.track($0) })
                index += run.count
            }
            return rows
        }
    }
}

// MARK: - Grouping

extension QueueItem {
    /// Which sleeve to draw for this row.
    ///
    /// The record wherever the library still knows it, so a queued album is one
    /// fetch and one cached bitmap rather than one of each per track on it.
    /// Anything the library has lost falls back to its own file.
    var sleeve: AlbumArtwork.Source? {
        if let albumId { return .album(albumId) }
        return trackId.map { .track($0) }
    }
}

struct QueueGroup: Identifiable {
    let id: String
    let albumArtist: String
    let album: String
    var items: [QueueItem]

    var year: String? { items.first?.year }

    var title: String {
        if !album.isEmpty { return album }
        return albumArtist.isEmpty ? "Unknown Artist" : albumArtist
    }

    /// "2007 · 11 tracks · 59:10 · FLAC". The codec only earns its place when
    /// the whole run shares one — a mixed group would be lying.
    var detail: String {
        var parts: [String] = []
        if let year, !year.isEmpty { parts.append(year) }
        parts.append(Format.count(Int64(items.count), "track"))
        let total = items.compactMap(\.durationMs).reduce(0, +)
        if total > 0 { parts.append(Format.duration(total)) }
        let codecs = Set(items.compactMap(\.codec))
        if let codec = codecs.first, codecs.count == 1 { parts.append(codec.uppercased()) }
        return parts.joined(separator: " · ")
    }
}

/// What the selection is, and what to do with it. The one reader of the
/// selection outside the List, so a click re-runs this and not the queue.
private struct QueueSelectionHeader: View {
    @Binding var selection: Set<String>
    let rows: [QueueView.Row]
    let remove: () -> Void

    @Environment(PlayerModel.self) private var player

    var body: some View {
        if !selection.isEmpty {
            // Each at its own width: squeezed by the rest of the header on a
            // phone, they wrap a few letters to a line.
            Text("\(QueueView.Row.itemIds(in: selection, of: rows).count) selected")
                .font(.role(.fine, system: .caption))
                .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                .lineLimit(1)
                .fixedSize()
            Group {
                Button { selection = [] } label: {
                    Label("Clear", systemImage: Icon.deselect)
                }
                Button(role: .destructive, action: remove) {
                    Label("Remove", systemImage: Icon.remove)
                }
            }
            .fixedSize()
            #if os(iOS)
            .labelStyle(.iconOnly)
            #else
            .labelStyle(.titleOnly)
            #endif
        }
        // Mirrored to the model for the Edit menu, which cannot reach a view's
        // state. The *queue item* ids, not the row ids: an album heading's id
        // is synthetic, and handing that to the engine gets it rejected as not
        // being a queue item.
        Color.clear.frame(width: 0, height: 0)
            .onChange(of: selection) { _, new in
                player.queueSelection = Set(QueueView.Row.itemIds(in: new, of: rows))
            }
    }
}

/// Jumps to what is playing and follows it from then on, tinted while it
/// does; pressed again, or any scroll of the person's own, stops following.
/// Beside the layout picker because both are about what you are looking at
/// rather than what is in the queue. Disabled rather than hidden when nothing
/// is playing: a control that comes and goes is one you have to look for. Its
/// own view because that read is of what is playing, which moves on every
/// pause and every edit.
private struct JumpToPlayingButton: View {
    @Environment(PlayerModel.self) private var player
    @Environment(UIState.self) private var ui

    var body: some View {
        let following = ui.followingQueue
        Button { ui.toggleFollowingQueue() } label: {
            // On a disc of the tint while following, so the state reads by
            // shape as well as colour.
            Image(systemName: Icon.jumpToPlaying)
                .foregroundStyle(following ? AnyShapeStyle(.tint) : KoanTheme.style(.ink, system: .primary))
                .padding(4)
                .background(
                    following ? AnyShapeStyle(.tint.opacity(0.18)) : AnyShapeStyle(.clear),
                    in: Circle()
                )
                .contentShape(Circle())
                .touchTarget()
        }
        #if os(iOS) || os(tvOS)
        // A default button tints its label on a phone whatever the label
        // asks for, which left the button lit after following stopped.
        .buttonStyle(.plain)
        #endif
        .disabled(player.currentItemId == nil)
        // Nothing playing is nothing to follow: the button shows that rather
        // than a tint it cannot be pressed to clear.
        .onChange(of: player.currentItemId == nil) { _, none in
            if none { ui.followingQueue = false }
        }
        .help(ui.followingQueue ? "Following what's playing; click to stop" : "Scroll to what's playing and follow it")
        .accessibilityAddTraits(following ? .isSelected : [])
    }
}

/// The playing row kept in view while following. Reads what is playing so the
/// queue's own body does not, which on a long queue would regroup and diff
/// every row on each change to it.
private struct FollowPlaying: View {
    let scroll: ScrollViewProxy

    @Environment(PlayerModel.self) private var player
    @Environment(EngineMirror.self) private var mirror
    @Environment(UIState.self) private var ui

    var body: some View {
        Color.clear
            .onChange(of: player.currentItemId) { _, _ in follow() }
            // The playing item and the rows arrive separately: a track played
            // from a new queue is scrolled to once it is listed.
            .onChange(of: mirror.queueVersion) { _, _ in follow() }
    }

    private func follow() {
        guard ui.followingQueue, let id = player.currentItemId, mirror.queueItem(id) != nil else { return }
        withAnimation(.easeInOut(duration: 0.3)) {
            scroll.scrollTo(id, anchor: .center)
        }
    }
}

/// "1,204 tracks · 3 days 2:10:05", as the queue reads now. Its own view, so a
/// patch re-runs this line and not the list.
private struct QueueSummary: View {
    @Environment(EngineMirror.self) private var mirror

    var body: some View {
        let queue = mirror.queue
        let total = queue.compactMap(\.durationMs).reduce(0, +)
        let count = Format.count(Int64(queue.count), "track")
        Text(total > 0 ? "\(count) · \(Format.duration(total))" : count)
    }
}

/// A track row of the list, reading its own state from the mirror, so that a
/// track change or a download redraws the rows on screen and not the list.
/// See `EngineMirror.queueRows`.
private struct LiveQueueRow: View {
    let sent: QueueItem
    let showArtist: Bool
    let artwork: Bool

    @Environment(EngineMirror.self) private var mirror

    var body: some View {
        let item = mirror.queueItem(sent.queueItemId) ?? sent
        QueueRow(
            item: QueueRowContent(item: item),
            // The queue already says which row the cursor is on — and says it
            // again when the cursor moves, since that redraws two rows either
            // way. Asking the player as well would subscribe every row to
            // everything else about what is playing.
            isCurrent: item.status == .playing,
            showArtist: showArtist,
            artwork: artwork
        )
    }
}

private struct QueueAlbumHeader: View {
    let group: QueueGroup

    // A fixed size reads as a heading on a desktop; a television scales the
    // text styles beneath it and left the record smaller than its artist.
    #if os(tvOS)
    private static let titleFont = Font.title3.weight(.semibold)
    private static let sleeve: CGFloat = 96
    #else
    private static let titleFont = Font.system(size: 14, weight: .semibold)
    private static let sleeve: CGFloat = 52
    #endif

    var body: some View {
        HStack(spacing: 12) {
            // No tap-to-view here, unlike the album page: this cover sits in a
            // selectable, draggable row, and a tap gesture on it would eat the
            // click that selects the row.
            if let sleeve = group.items.first?.sleeve {
                AlbumArtwork(source: sleeve, size: .thumb, cornerRadius: KoanTheme.radius(5))
                    .frame(width: Self.sleeve, height: Self.sleeve)
                    .koanShadow(0.28, radius: 4, y: 2)
            }

            VStack(alignment: .leading, spacing: 2) {
                Text(Format.title(group.title))
                    .font(.role(.titleSmall, system: Self.titleFont))
                    .foregroundStyle(KoanTheme.style(.ink, system: .primary))
                    .lineLimit(Format.titleLines)

                // Only when the line above is the record: a group with no album
                // title already leads with the artist.
                if !group.album.isEmpty {
                    Text(group.albumArtist.isEmpty ? "Unknown Artist" : group.albumArtist)
                        .font(.role(.fine, system: .caption))
                        .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                        .lineLimit(1)
                }

                Text(group.detail)
                    .font(.role(.fine, system: .caption2.monospacedDigit()))
                    .foregroundStyle(KoanTheme.style(.muted, system: .tertiary))
                    .lineLimit(1)
            }

            Spacer()
        }
        .textCase(nil)
        .padding(.vertical, 6)
    }
}
