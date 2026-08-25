import SwiftUI

/// A menu-bar shortcut.
///
/// The menus are built from these and so is the shortcuts sheet, so what the
/// app does and what it tells you it does cannot drift. Bare keys live in
/// `Hotkeys`; anything carrying a modifier belongs here, because a menu is
/// where macOS expects to find it and the sheet is only writing down what the
/// menu bar already says.
struct MenuShortcut: Identifiable {
    let title: String
    let icon: String
    let key: KeyEquivalent
    let modifiers: EventModifiers
    let group: Hotkey.Group

    var id: String { title }

    /// ⇧⌘K, ⌥←, ⌫ — in the order macOS prints them.
    var caption: String {
        var out = ""
        if modifiers.contains(.control) { out += "⌃" }
        if modifiers.contains(.option) { out += "⌥" }
        if modifiers.contains(.shift) { out += "⇧" }
        if modifiers.contains(.command) { out += "⌘" }
        return out + Self.glyph(key)
    }

    private static func glyph(_ key: KeyEquivalent) -> String {
        switch key.character {
        case KeyEquivalent.leftArrow.character: "←"
        case KeyEquivalent.rightArrow.character: "→"
        case KeyEquivalent.upArrow.character: "↑"
        case KeyEquivalent.downArrow.character: "↓"
        case KeyEquivalent.delete.character: "⌫"
        case KeyEquivalent.return.character: "↩"
        case " ": "space"
        default: String(key.character).uppercased()
        }
    }
}

/// A sidebar section reachable by number.
struct NavigationCommand {
    let title: String
    let icon: String
    let key: KeyEquivalent
    let section: Navigator.Section

    /// Same symbols the sidebar uses — the View menu and the sidebar are two
    /// ways to the same page.
    static let all: [NavigationCommand] = [
        .init(title: "Queue", icon: Icon.queueSection, key: "1", section: .queue),
        .init(title: "Albums", icon: Icon.album, key: "2", section: .albums),
        .init(title: "Artists", icon: Icon.artist, key: "3", section: .artists),
        .init(title: "Favourites", icon: Icon.favourite, key: "4", section: .favourites),
        .init(title: "History", icon: Icon.history, key: "5", section: .playHistory),
    ]

    var shortcut: MenuShortcut {
        MenuShortcut(title: title, icon: icon, key: key, modifiers: .command, group: .navigation)
    }
}

extension MenuShortcut {
    static let search = Self(
        title: "Search…", icon: Icon.search, key: "k", modifiers: .command, group: .navigation)
    static let addMusic = Self(
        title: "Add Music…", icon: Icon.add, key: "k", modifiers: [.command, .shift],
        group: .navigation)
    static let back = Self(
        title: "Back", icon: Icon.back, key: "[", modifiers: .command, group: .navigation)
    static let forward = Self(
        title: "Forward", icon: Icon.forward, key: "]", modifiers: .command, group: .navigation)

    static let next = Self(
        title: "Next", icon: Icon.next, key: .rightArrow, modifiers: .command, group: .playback)
    static let previous = Self(
        title: "Previous", icon: Icon.previous, key: .leftArrow, modifiers: .command,
        group: .playback)
    static let skipForward = Self(
        title: "Skip Forward", icon: Icon.skipForward, key: .rightArrow, modifiers: .option,
        group: .playback)
    static let skipBack = Self(
        title: "Skip Back", icon: Icon.skipBack, key: .leftArrow, modifiers: .option,
        group: .playback)
    static let favourite = Self(
        title: "Favourite Current Track", icon: Icon.favourite, key: "d", modifiers: .command,
        group: .playback)
    static let radio = Self(
        title: "Toggle Radio", icon: Icon.radio, key: "r", modifiers: [.command, .option],
        group: .playback)

    static let lyrics = Self(
        title: "Toggle Lyrics", icon: Icon.lyrics, key: "l", modifiers: [.command, .option],
        group: .view)
    static let shortcuts = Self(
        title: "Keyboard Shortcuts", icon: Icon.shortcuts, key: "/", modifiers: .command,
        group: .view)

    static let undo = Self(
        title: "Undo", icon: Icon.undo, key: "z", modifiers: .command, group: .edit)
    static let redo = Self(
        title: "Redo", icon: Icon.redo, key: "z", modifiers: [.command, .shift], group: .edit)
    static let cut = Self(
        title: "Cut", icon: Icon.cut, key: "x", modifiers: .command, group: .edit)
    static let copy = Self(
        title: "Copy", icon: Icon.copy, key: "c", modifiers: .command, group: .edit)
    static let paste = Self(
        title: "Paste", icon: Icon.paste, key: "v", modifiers: .command, group: .edit)
    static let delete = Self(
        title: "Delete", icon: Icon.remove, key: .delete, modifiers: [], group: .edit)
    static let selectAll = Self(
        title: "Select All", icon: Icon.selectAll, key: "a", modifiers: .command, group: .edit)
    static let find = Self(
        title: "Find", icon: Icon.search, key: "f", modifiers: .command, group: .edit)

    static let rescan = Self(
        title: "Rescan Local Folders", icon: Icon.rescan, key: "r", modifiers: [.command, .shift],
        group: .library)

    /// Everything with a modifier, in the order the sheet lists it.
    /// Built a line at a time: one long `+` chain of array literals is what the
    /// type checker gives up on.
    static let all: [MenuShortcut] = {
        var all: [MenuShortcut] = [search, addMusic]
        all += NavigationCommand.all.map(\.shortcut)
        all += [back, forward]
        all += [next, previous, skipForward, skipBack, favourite, radio]
        all += [lyrics, shortcuts]
        all += [undo, redo, cut, copy, paste, delete, selectAll, find]
        all.append(rescan)
        return all
    }()
}

/// A menu item that takes its title and its key from the same place the sheet
/// reads them.
struct ShortcutButton: View {
    let shortcut: MenuShortcut
    let action: () -> Void

    init(_ shortcut: MenuShortcut, action: @escaping () -> Void) {
        self.shortcut = shortcut
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            Label(shortcut.title, systemImage: shortcut.icon)
        }
        .keyboardShortcut(shortcut.key, modifiers: shortcut.modifiers)
    }
}
