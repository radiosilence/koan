#if os(macOS)
import AppKit
import KoanFFI
import SwiftUI

/// The sidebar's search in the theme: the theme's field, and the suggestions
/// in a square panel under it, laid over the sidebar's rows, which start
/// `fieldHeight` down. Drawn in the window
/// rather than as a child panel, so the field keeps the keyboard while the
/// suggestions follow what is typed. The arrow keys move through them, Return
/// takes the one lit or, with none lit, opens every result, and Escape
/// empties the field. `/` reaches it through `UIState.searchFocusToken`.
struct SidebarSearch: View {
    @Environment(SearchModel.self) private var search
    @Environment(UIState.self) private var ui
    @FocusState private var focused: Bool
    @Binding var fieldHeight: CGFloat
    @State private var lit: Int?
    /// The pointer is over the panel: a click there may take the keyboard
    /// from the field on mouse-down, and the panel stays for the click to land.
    @State private var pointing = false

    var body: some View {
        let items = suggestions

        VStack(spacing: 0) {
            field(items)
            if focused || pointing, !search.queryIsToken, !items.isEmpty {
                panel(items).padding(.horizontal, KoanTheme.Space.s)
            }
        }
    }

    private func field(_ items: [Suggestion]) -> some View {
        @Bindable var search = search
        return KoanSearchField(text: $search.query, prompt: "Search", focus: $focused) {
            if let lit, items.indices.contains(lit) {
                pick(items[lit])
            } else {
                search.submit()
            }
        }
        .padding(.horizontal, KoanTheme.Space.s)
        .padding(.vertical, KoanTheme.Space.s)
        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { fieldHeight = $0 }
        .onKeyPress(.downArrow) {
            guard !items.isEmpty, !composing else { return .ignored }
            lit = min((lit ?? -1) + 1, items.count - 1)
            return .handled
        }
        .onKeyPress(.upArrow) {
            guard let current = lit, !composing else { return .ignored }
            lit = current == 0 ? nil : current - 1
            return .handled
        }
        .onKeyPress(.escape) {
            guard !search.query.isEmpty, !composing else { return .ignored }
            search.query = ""
            return .handled
        }
        .onChange(of: search.query) { _, _ in lit = nil }
        .onChange(of: ui.searchFocusToken) { _, _ in focused = true }
    }

    private func panel(_ items: [Suggestion]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(items.enumerated()), id: \.element.token) { index, item in
                if index == 0 || items[index - 1].kind != item.kind {
                    Text(item.kind)
                        .koanText(.fine, .muted)
                        .accessibilityAddTraits(.isHeader)
                        .padding(.horizontal, KoanTheme.Space.s)
                        .padding(.top, KoanTheme.Space.s)
                        .padding(.bottom, KoanTheme.Space.xs)
                }
                SuggestionLine(item: item, lit: lit == index)
                    .onHover { if $0 { lit = index } }
                    .onTapGesture { pick(item) }
                    .accessibilityElement(children: .combine)
                    .accessibilityAddTraits(lit == index ? [.isButton, .isSelected] : .isButton)
                    .accessibilityAction { pick(item) }
            }
        }
        .padding(.bottom, KoanTheme.Space.xs)
        .frame(maxWidth: .infinity, alignment: .leading)
        .koanPopover()
        .overlay { Rectangle().strokeBorder(Color.koanRule, lineWidth: KoanTheme.hairline) }
        .onHover { pointing = $0 }
        .onDisappear { pointing = false }
    }

    private func pick(_ item: Suggestion) {
        search.query = item.token
        search.submit()
        focused = false
        pointing = false
    }

    /// An input method is composing: its candidate window takes the arrows
    /// and Escape, not the suggestions.
    private var composing: Bool {
        (NSApp.keyWindow?.firstResponder as? NSTextView)?.hasMarkedText() == true
    }

    /// The same few the system's dropdown offered: five tracks, four records,
    /// four artists.
    private var suggestions: [Suggestion] {
        search.tracks.prefix(5).map {
            Suggestion(
                kind: "tracks", albumId: $0.albumId, title: $0.title,
                subtitle: "\($0.artistName) — \($0.albumTitle)",
                token: SearchModel.Selection.track($0.id, album: $0.albumId).token
            )
        } + search.albums.prefix(4).map {
            Suggestion(
                kind: "albums", albumId: $0.id, title: $0.title, subtitle: $0.artistName,
                token: SearchModel.Selection.album($0.id).token
            )
        } + search.artists.prefix(4).map {
            Suggestion(
                kind: "artists", albumId: nil, title: $0.name, subtitle: nil,
                token: SearchModel.Selection.artist($0.id).token
            )
        }
    }
}

private struct Suggestion {
    let kind: String
    let albumId: Int64?
    let title: String
    let subtitle: String?
    let token: String
}

private struct SuggestionLine: View {
    let item: Suggestion
    let lit: Bool

    var body: some View {
        HStack(spacing: KoanTheme.Space.s) {
            Group {
                if let albumId = item.albumId {
                    AlbumArtwork(source: .album(albumId), size: .thumb, cornerRadius: KoanTheme.radius(3))
                } else {
                    KoanIcon(Icon.artist).foregroundStyle(Color.koanMuted)
                }
            }
            .frame(width: 28, height: 28)
            VStack(alignment: .leading, spacing: 1) {
                Text(item.title)
                    .koanText(.meta)
                    .lineLimit(1)
                if let subtitle = item.subtitle {
                    Text(subtitle)
                        .koanText(.fine, .muted)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, KoanTheme.Space.s)
        .padding(.vertical, KoanTheme.Space.xs)
        .background(lit ? Color.koanSurface : Color.clear)
        .contentShape(Rectangle())
    }
}
#endif
