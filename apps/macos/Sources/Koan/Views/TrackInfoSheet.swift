import KoanFFI
import SwiftUI

/// Opens a track's info view. Put in the environment by `trackInfoHost()`;
/// absent where there is no host, which is where no menu offers Info.
struct ShowTrackInfo {
    let action: @MainActor (Int64) -> Void

    @MainActor func callAsFunction(_ trackId: Int64) { action(trackId) }
}

extension EnvironmentValues {
    @Entry var showTrackInfo: ShowTrackInfo?
}

/// "Info…", for any menu that names one library track.
struct TrackInfoButton: View {
    let trackId: Int64

    @Environment(\.showTrackInfo) private var showTrackInfo

    var body: some View {
        if let showTrackInfo {
            Button { showTrackInfo(trackId) } label: {
                Label("Info…", koan: Icon.info)
            }
        }
    }
}

/// The one place a track's info is presented from a menu.
///
/// Attached to the window rather than to the menu that asked, for the reason
/// `NewPlaylistAlert` gives: a context menu is gone the instant you pick from
/// it, and takes anything it presents with it. A sheet that shows its own
/// track's info, as Now Playing does, sets `showTrackInfo` for itself.
struct TrackInfoHost: ViewModifier {
    @State private var shown: Shown?

    struct Shown: Identifiable {
        let id: Int64
    }

    func body(content: Content) -> some View {
        #if os(tvOS)
        content
        #else
        content
            .environment(\.showTrackInfo, ShowTrackInfo { shown = Shown(id: $0) })
            #if os(iOS)
            .tray(isPresented: Binding(get: { shown != nil }, set: { if !$0 { shown = nil } })) {
                if let shown { TrackInfoLoader(trackId: shown.id) }
            }
            #else
            .sheet(item: $shown) { shown in
                NavigationStack {
                    ScrollView { TrackInfoLoader(trackId: shown.id) }
                        .toolbar {
                            KoanSheetAction(placement: .confirmationAction) {
                                Button("Done") { self.shown = nil }
                            }
                        }
                }
                .koanSheetStack()
                .resizableWindow("TrackInfo", min: CGSize(width: 420, height: 360), ideal: CGSize(width: 560, height: 720))
            }
            #endif
        #endif
    }
}

/// Reads a track's details, then shows them.
private struct TrackInfoLoader: View {
    let trackId: Int64

    @Environment(LibraryModel.self) private var library
    @State private var info: TrackDetails?
    @State private var loaded = false

    var body: some View {
        Group {
            if let info {
                TrackInfoView(info: info)
            } else if loaded {
                Text("This track is no longer in the library.")
                    .koanText(.fine, .muted)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(14)
            }
        }
        .task(id: trackId) {
            info = (try? await library.engine.trackDetails(trackId: trackId)) ?? nil
            loaded = true
        }
    }
}

extension View {
    /// Hosts the info view menus open. Once per window, high enough up that no
    /// menu that opens it can take it away again.
    func trackInfoHost() -> some View {
        modifier(TrackInfoHost())
    }
}
