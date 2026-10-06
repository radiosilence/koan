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
        if KoanTheme.isOn {
            let warm = KoanAccent(record: Color(red: 0.94, green: 0.54, blue: 0.36))
            let navy = KoanAccent(record: Color(red: 0.04, green: 0.10, blue: 0.23))
            for (name, accent) in [("mint", KoanAccent.mint), ("warm", warm), ("navy", navy)] {
                pages.append(("theme-\(name)", CGSize(width: 760, height: 1100), AnyView(
                    KoanThemeSheet(accent: accent)
                )))
            }
            pages.append(("theme-no-icons", CGSize(width: 760, height: 1100), AnyView(
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
        NSApp.terminate(nil)
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
#endif
