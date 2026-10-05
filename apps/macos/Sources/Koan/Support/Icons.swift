/// The symbol for an action, named once.
///
/// The same verb turns up as a toolbar button, a context-menu item and a
/// menu-bar command. Naming the symbol here is what keeps them the same action.
///
/// Two removals, deliberately distinct: taking rows out of a list is
/// `remove`, and emptying the whole thing is `clear`.
enum Icon {
    static let play = "play.fill"
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

    static let favourite = "heart"
    static let favourited = "heart.fill"
    static let share = "link"
    static let organize = "folder.badge.gearshape"
    static let remove = "minus.circle"
    static let deselect = "xmark.circle"
    static let clear = "trash"

    static let album = "square.stack"
    static let artist = "music.mic"
    static let track = "music.note"
    static let queueSection = "list.bullet"
    /// Put the queue back on the row that is playing.
    static let jumpToPlaying = "scope"
    static let history = "clock.arrow.circlepath"
    static let recentlyPlayed = "clock"
    static let downloads = "arrow.down.circle"
    static let playlist = "music.note.list"
    static let search = "magnifyingglass"
    static let add = "plus"
    static let back = "chevron.left"
    static let forward = "chevron.right"
    static let lyrics = "quote.bubble"
    static let shortcuts = "keyboard"

    static let undo = "arrow.uturn.backward"
    static let redo = "arrow.uturn.forward"
    static let cut = "scissors"
    static let copy = "doc.on.doc"
    static let paste = "doc.on.clipboard"
    static let selectAll = "checkmark.circle"

    static let save = "square.and.arrow.down"
    static let rescan = "arrow.clockwise"
    static let rescanAll = "arrow.clockwise.circle"
    static let sync = "arrow.triangle.2.circlepath"
}
