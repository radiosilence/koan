import KoanFFI
import SwiftUI

/// Where a track's bytes are: on the server, on this machine, or both.
///
/// Both, because they are not exclusive. A record that exists as local files
/// *and* on the server is one row that is genuinely both, and the old single
/// "source" reading meant the cloud vanished the moment anything was
/// downloaded — which read as the track having left the server.
///
/// Shared by every view that lists tracks so the vocabulary is the same
/// wherever you meet it.
struct SourceBadges: View {
    let onServer: Bool
    let onDisk: Bool

    var body: some View {
        HStack(spacing: 3) {
            if onServer {
                Image(systemName: "cloud")
                    .foregroundStyle(.quaternary)
                    .help(onDisk ? "On your server, downloaded" : "On your server — downloads on play")
            }
            if onDisk {
                Image(systemName: "internaldrive")
                    .foregroundStyle(.tertiary)
                    .help(onServer ? "Downloaded to this machine" : "Local file")
            }
        }
        .font(.caption2)
        .imageScale(.small)
    }
}

extension SourceBadges {
    init(track: Track) {
        self.init(onServer: track.onServer, onDisk: track.onDisk)
    }
}
