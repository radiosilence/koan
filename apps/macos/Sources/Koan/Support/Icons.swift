import SwiftUI

/// The symbol for an action or a thing, named once.
///
/// The same verb turns up as a toolbar button, a context-menu item and a
/// menu-bar command. Naming the symbol here is what keeps them the same action.
/// Each value is an SF Symbol's name, which the platform's look draws; the kōan
/// theme draws the glyph of its own set that stands in for it (`KoanGlyph`).
/// Views ask for one through `KoanIcon`, `KoanLabel`, `Image(koan:)`,
/// `Label(_:koan:)` or `Button(_:koan:)`, never through `Image(systemName:)`.
///
/// Two removals, deliberately distinct: taking rows out of a list is
/// `remove`, and emptying the whole thing is `clear`.
enum Icon {
    static let play = "play.fill"
    static let pause = "pause.fill"
    static let playPause = "playpause.fill"
    static let playNext = "text.line.first.and.arrowtriangle.forward"
    static let queue = "text.append"
    static let shuffle = "shuffle"
    static let repeatQueue = "repeat"
    static let repeatOne = "repeat.1"
    /// Deal the album grid in a new random order. Playback is `shuffle`.
    static let reshuffle = "dice"
    static let next = "forward.fill"
    static let previous = "backward.fill"
    static let skipForward = "goforward.10"
    static let skipBack = "gobackward.10"
    static let sleep = "moon"
    static let sleepSet = "moon.zzz.fill"
    /// Play this, over a sleeve or at the head of a row.
    static let playMark = "play.circle.fill"
    static let nowPlaying = "play.circle"

    static let favourite = "heart"
    static let favourited = "heart.fill"
    static let share = "link"
    static let organize = "folder.badge.gearshape"
    static let remove = "minus.circle"
    static let deselect = "xmark.circle"
    static let clear = "trash"
    static let close = "xmark"
    /// Empty a field.
    static let clearField = "xmark.circle.fill"

    static let album = "square.stack"
    static let unknownAlbum = "questionmark.square.dashed"
    static let artist = "music.mic"
    static let track = "music.note"
    static let queueSection = "list.bullet"
    /// Put the queue back on the row that is playing.
    static let jumpToPlaying = "scope"
    static let history = "clock.arrow.circlepath"
    static let recentlyPlayed = "clock"
    static let onDevice = "internaldrive"
    static let downloads = "arrow.down.circle"
    static let downloadNotice = "arrow.down.circle.fill"
    static let playlist = "music.note.list"
    static let library = "music.note.house"
    static let search = "magnifyingglass"
    static let add = "plus"
    static let subtract = "minus"
    static let back = "chevron.left"
    static let forward = "chevron.right"
    static let disclosure = "chevron.right"
    static let expand = "chevron.down"
    static let choose = "chevron.up.chevron.down"
    /// The field before and after, above a keyboard.
    static let previousField = "chevron.up"
    static let nextField = "chevron.down"
    static let lyrics = "quote.bubble"
    static let lyricsText = "text.quote"
    static let shortcuts = "keyboard"
    static let sidebar = "sidebar.left"
    static let sort = "arrow.up.arrow.down"
    static let filters = "slider.horizontal.3"
    static let more = "ellipsis"
    static let moreCircled = "ellipsis.circle"

    static let undo = "arrow.uturn.backward"
    static let redo = "arrow.uturn.forward"
    static let cut = "scissors"
    static let copy = "doc.on.doc"
    static let paste = "doc.on.clipboard"
    static let selectAll = "checkmark.circle"
    /// Something done as asked.
    static let success = "checkmark.circle"
    /// Select mode, off and on.
    static let select = "square"
    static let selecting = "checkmark.square.fill"
    /// A tile or row's tick, picked and not.
    static let picked = "checkmark.circle.fill"
    static let unpicked = "circle"
    static let check = "checkmark"
    static let moveUp = "arrow.up"
    static let moveDown = "arrow.down"
    static let move = "arrow.right"

    static let save = "square.and.arrow.down"
    static let export = "square.and.arrow.up"
    static let openExternal = "arrow.up.right.square"
    static let mail = "envelope"
    static let rescan = "arrow.clockwise"
    static let rescanAll = "arrow.clockwise.circle"
    static let sync = "arrow.triangle.2.circlepath"
    static let folder = "folder"
    static let folderUnknown = "folder.badge.questionmark"

    static let pending = "circle.dotted"
    static let warning = "exclamationmark.triangle"
    static let warningFilled = "exclamationmark.triangle.fill"
    static let alert = "exclamationmark.circle.fill"
    static let failure = "xmark.octagon"
    static let failureFilled = "xmark.octagon.fill"
    static let info = "info.circle"
    static let cloud = "cloud"
    static let cloudKept = "cloud.fill"
    static let cloudMissing = "icloud.slash"
    static let cloudProblem = "exclamationmark.icloud"
    static let wifi = "wifi"
    static let offline = "wifi.slash"
    static let networkBlocked = "wifi.exclamationmark"

    static let renderer = "antenna.radiowaves.left.and.right"
    static let speaker = "hifispeaker"
    static let output = "speaker.wave.2"
    static let volumeDown = "speaker.fill"
    static let volumeUp = "speaker.wave.3.fill"
    static let laptop = "laptopcomputer"
    static let desktop = "desktopcomputer"
    static let display = "display"
    static let phone = "iphone"
    static let television = "appletv"
    static let cable = "cable.connector"
    static let headphones = "headphones"
    static let airplay = "airplayaudio"
    static let server = "server.rack"

    static let settings = "gearshape"
    static let devices = "laptopcomputer.and.iphone"
    static let account = "person.crop.circle"
    static let people = "person.2"
    static let tuning = "slider.vertical.3"
    static let extensions = "puzzlepiece.extension"
    static let appearance = "paintpalette"

    /// The point size an icon is drawn at where nothing sets one: a menu, a
    /// toolbar, a segment. The platform's body size.
    #if os(macOS)
    static let pointSize: CGFloat = 13
    #elseif os(tvOS)
    static let pointSize: CGFloat = 29
    #else
    static let pointSize: CGFloat = 17
    #endif
}

extension Image {
    /// An `Icon` at a fixed size, for the places that take only an `Image`:
    /// menus, toolbar items, segments. The kōan glyph in the theme, the SF
    /// Symbol in the platform's look. Elsewhere `KoanIcon` follows the font.
    @MainActor
    init(koan icon: String, pointSize: CGFloat = Icon.pointSize, layer: KoanGlyph.Layer = .all) {
        guard KoanTheme.isOn, let glyph = KoanGlyph.forSymbol(icon) else {
            self.init(systemName: icon) // theme: raw
            return
        }
        #if os(macOS)
        self = Image(nsImage: glyph.dynamicImage(pointSize: pointSize, layer: layer)).renderingMode(.template)
        #else
        self = Image(uiImage: glyph.image(pointSize: pointSize, layer: layer)).renderingMode(.template)
        #endif
    }
}

extension Label where Title == Text, Icon == Image {
    /// `Label(_:systemImage:)` through the theme's icons.
    @MainActor
    init(_ title: some StringProtocol, koan icon: String) {
        self.init { Text(title) } icon: { Image(koan: icon) }
    }
}

extension Button where Label == SwiftUI.Label<Text, Image> {
    /// `Button(_:systemImage:role:action:)` through the theme's icons.
    @MainActor
    init(_ title: some StringProtocol, koan icon: String, role: ButtonRole? = nil, action: @escaping @MainActor () -> Void) {
        self.init(role: role, action: action) { SwiftUI.Label(title, koan: icon) }
    }
}
