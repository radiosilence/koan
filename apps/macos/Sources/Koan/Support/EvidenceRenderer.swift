#if os(macOS)
import AppKit
import SwiftUI

/// Pictures of pages for a pull request, drawn by the app itself.
///
/// With `KOAN_RENDER_EVIDENCE` set to a directory, the app renders the pages
/// below, light and dark, into PNGs there once the engine is up, and quits.
/// Each is drawn in a window of its own placed off every screen and never
/// shown, so nothing reaches the display, and no Screen Recording permission
/// is needed, as a capture from another process would. Run it against a
/// scratch `KOAN_CONFIG_DIR` that holds what the pages should show.
@MainActor
enum EvidenceRenderer {
    static var directory: URL? {
        ProcessInfo.processInfo.environment["KOAN_RENDER_EVIDENCE"]
            .map { URL(fileURLWithPath: $0, isDirectory: true) }
    }

    static func run(_ state: AppState, into dir: URL) async {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        state.dsp.reload()
        // The overview and each page's own reads arrive asynchronously.
        try? await Task.sleep(for: .seconds(2))

        var pages: [(name: String, size: CGSize, view: AnyView)] = [
            ("eq", CGSize(width: 620, height: 760), AnyView(
                EqSettings().environment(state)
            )),
        ]
        for profile in state.dsp.overview?.profiles ?? [] {
            let slug = profile.name.lowercased()
                .map { $0.isLetter || $0.isNumber ? String($0) : "-" }
                .joined()
            pages.append(("dsp-profile-\(slug)", CGSize(width: 620, height: 900), AnyView(
                NavigationStack { DspProfilePage(dsp: state.dsp, name: profile.name) }
            )))
        }
        // The transport's popovers, as their content: a popover is not drawn in
        // a window that is never shown.
        // In the room's accent, as they open over the transport.
        var sleeve: Color?
        if let source = state.player.currentArtwork { sleeve = await state.art.dominantColour(for: source) }
        let room = KoanAccent.of(sleeve)
        pages.append(("popover-output", CGSize(width: 340, height: 420), AnyView(
            OutputPicker().koanPopover().appEnvironment(state)
                .tint(room.color).environment(\.koanAccent, room).environment(\.roomTint, room.color)
        )))
        pages.append(("popover-control", CGSize(width: 340, height: 320), AnyView(
            ControlPicker().koanPopover().appEnvironment(state)
                .tint(room.color).environment(\.koanAccent, room).environment(\.roomTint, room.color)
        )))
        if KoanTheme.isOn {
            let warm = KoanAccent.of(Color(red: 0.94, green: 0.54, blue: 0.36)) // theme: raw — a sleeve's colour, as input
            let navy = KoanAccent.of(Color(red: 0.04, green: 0.10, blue: 0.23)) // theme: raw — a sleeve's colour, as input
            for (name, accent) in [("mint", KoanAccent.mint), ("warm", warm), ("navy", navy)] {
                pages.append(("theme-\(name)", CGSize(width: 760, height: 1400), AnyView(
                    KoanThemeSheet(accent: accent)
                )))
            }
            pages.append(("theme-no-icons", CGSize(width: 760, height: 1400), AnyView(
                KoanThemeSheet().environment(\.koanIcons, false)
            )))
        }
        pages += await SettingsEvidence.pages(state)
        pages.append(("sheet-shortcuts", CGSize(width: 680, height: 620), AnyView(
            ShortcutsSheet(hotkeys: state.hotkeys.all).koanTheme(state.appearance)
        )))
        // `KOAN_RENDER_PAGES`, a comma-separated list of name prefixes, narrows
        // the run to the pages a pull request changes.
        if let only = ProcessInfo.processInfo.environment["KOAN_RENDER_PAGES"]?.split(separator: ",") {
            pages.removeAll { page in !only.contains { page.name.hasPrefix($0) } }
        }
        for page in pages {
            for dark in [false, true] {
                let file = dir.appending(path: "\(page.name)-\(dark ? "dark" : "light").png")
                await snapshot(page.view, size: page.size, dark: dark, to: file)
            }
        }
        // The whole window, at a section: the shell, the room and the page
        // together. What is playing is whatever the scratch library's saved
        // session left cued.
        let nav = state.nav
        state.ui.showLyrics = false
        var windows: [(name: String, go: () -> Void)] = [
            ("window-queue", { nav.show(.queue) }),
            ("window-albums", { nav.show(.albums) }),
            ("window-artists", { nav.show(.artists) }),
            ("window-tracks", { nav.show(.tracks) }),
            ("window-favourites", { nav.show(.favourites) }),
            ("window-recent", { nav.show(.recentlyPlayed) }),
            ("window-history", { nav.show(.playHistory) }),
            ("window-downloads", { nav.show(.downloads) }),
            ("window-lyrics", { nav.show(.queue); state.ui.showLyrics = true }),
        ]
        if let album = state.player.currentAlbumId {
            windows.append(("window-album", { nav.open(album: album) }))
        }
        // The sidebar on its own as well: a split view's sidebar column is not
        // drawn in a window that is never shown.
        windows.append(("window-sidebar", { nav.show(.albums) }))
        if let only = ProcessInfo.processInfo.environment["KOAN_RENDER_PAGES"]?.split(separator: ",") {
            windows.removeAll { window in !only.contains { window.name.hasPrefix($0) } }
        }
        // `KOAN_RENDER_FRAMED`: the window as a window — titlebar, toolbar,
        // sidebar column and all — at the size given (points, `1200x750` by
        // default): what pages look like in the window people see.
        let framed = ProcessInfo.processInfo.environment["KOAN_RENDER_FRAMED"].map { spec -> CGSize in
            let parts = spec.split(separator: "x").compactMap { Double($0) }
            return parts.count == 2 ? CGSize(width: parts[0], height: parts[1]) : CGSize(width: 1200, height: 750)
        }
        // One window for every page, as the app has: tearing one down under a
        // page still settling its geometry is a crash, not a picture.
        let frame = framed.map {
            FramedWindow(
                RootView(hotkeys: state.hotkeys).appEnvironment(state).environment(\.drawnOffscreen, true),
                size: $0
            )
        }
        // `KOAN_RENDER_SCHEMES=dark` (or `light`) draws one appearance only.
        let schemes = ProcessInfo.processInfo.environment["KOAN_RENDER_SCHEMES"].map {
            $0 == "dark" ? [true] : [false]
        } ?? [false, true]
        for window in windows {
            window.go()
            for dark in schemes {
                let file = dir.appending(path: "\(window.name)-\(dark ? "dark" : "light").png")
                if let frame {
                    await frame.capture(dark: dark, to: file)
                    continue
                }
                let sidebar = window.name == "window-sidebar"
                await snapshot(
                    sidebar
                        ? AnyView(SidebarView().koanSurface().appEnvironment(state))
                        : AnyView(RootView(hotkeys: state.hotkeys).appEnvironment(state).environment(\.drawnOffscreen, true)),
                    size: sidebar ? CGSize(width: 240, height: 900) : CGSize(width: 1440, height: 900),
                    dark: dark, to: file
                )
            }
        }
        NSApp.terminate(nil)
    }

    /// A real window, ordered in far off every screen so nobody sees it: the
    /// titlebar, the toolbar SwiftUI bridges into it and the split view's
    /// sidebar column only draw in a window that is ordered in. Read back from
    /// the window server as this process's own window; nothing is captured
    /// from the screen.
    @MainActor
    private final class FramedWindow {
        private let window: Unconstrained

        init(_ view: some View, size: CGSize) {
            let host = NSHostingController(
                rootView: view
                    .tint(.koanAccent)
                    .environment(\.controlActiveState, .key)
                    .environment(\.koanIcons, true)
            )
            host.sceneBridgingOptions = [.toolbars, .title]
            let place = CGRect(x: -30_000, y: -30_000, width: size.width, height: size.height)
            window = Unconstrained(
                contentRect: place,
                styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                backing: .buffered,
                defer: false
            )
            window.isReleasedWhenClosed = false
            window.contentViewController = host
            window.setFrame(place, display: false)
            window.orderFrontRegardless()
        }

        func capture(dark: Bool, to file: URL) async {
            window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
            try? await Task.sleep(for: .seconds(2.5))
            window.displayIfNeeded()
            guard let image = EvidenceRenderer.ownWindowImage(window.windowNumber) else { return }
            try? NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])?.write(to: file)
        }
    }

    /// This process's own window, as the window server composited it —
    /// vibrancy, the sidebar's material and all, which `cacheDisplay` leaves
    /// out. An app may read its own windows without screen-recording access.
    /// Looked up by name: the call is deprecated in favour of ScreenCaptureKit,
    /// which asks for that access even for an app's own windows.
    fileprivate static func ownWindowImage(_ number: Int) -> CGImage? {
        typealias Capture = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
        guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") else {
            return nil
        }
        let capture = unsafeBitCast(symbol, to: Capture.self)
        // .null bounds: the window's own; including window; best resolution, no shadow.
        let options: UInt32 = (1 << 3) | (1 << 0)
        return capture(.null, 1 << 3, UInt32(number), options)?.takeRetainedValue()
    }

    /// A window AppKit does not pull back onto a screen when it is ordered in.
    private final class Unconstrained: NSWindow {
        override func constrainFrameRect(_ rect: NSRect, to screen: NSScreen?) -> NSRect { rect }
    }

    private static func snapshot(_ view: AnyView, size: CGSize, dark: Bool, to file: URL) async {
        // Drawn as in the front window, as a person sees the page: a window
        // never shown is never key, and inactive controls lose their accent.
        let host = NSHostingView(
            rootView: view
                .tint(.koanAccent)
                .environment(\.controlActiveState, .key)
                .environment(\.koanIcons, true)
        )
        host.frame = CGRect(origin: .zero, size: size)
        // Far off every screen, and never ordered in: drawn, never shown.
        let window = NSWindow(
            contentRect: CGRect(x: -30_000, y: -30_000, width: size.width, height: size.height),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        // Held by this function, not released by `close()` as well.
        window.isReleasedWhenClosed = false
        window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        window.contentView = host
        // Each page's own `.task` reads what it shows.
        try? await Task.sleep(for: .seconds(1.5))
        host.layoutSubtreeIfNeeded()
        host.display()
        guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { return }
        host.cacheDisplay(in: host.bounds, to: rep)
        try? rep.representation(using: .png, properties: [:])?.write(to: file)
        window.contentView = nil
        window.close()
    }
}

extension View {
    /// What the main window's root is handed. `KoanApp` and the renderer both
    /// call this, so a page drawn here is drawn as the window draws it.
    func appEnvironment(_ state: AppState) -> some View {
        environment(state)
            .environment(state.ui)
            .environment(state.player)
            .environment(state.library)
            .environment(state.nav)
            .environment(state.search)
            .environment(state.art)
            .environment(state.organize)
            .environment(state.playlists)
            .environment(state.activity)
            .environment(state.levels)
            .environment(state.meter)
            .environment(state.mirror)
            .koanTheme(state.appearance)
    }
}

#endif
