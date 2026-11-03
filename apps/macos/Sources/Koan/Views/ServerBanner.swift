import KoanFFI
import SwiftUI

/// The server this device cannot use, and why, across the top of every page
/// for as long as that holds. Other devices see this one as unreachable and
/// their commands are refused, so the reason belongs on its own screen.
/// Offline by choice shows nothing; the engine decides (`ServerStanding`).
struct ServerBanner: View {
    @Environment(EngineMirror.self) private var mirror
    let signIn: () -> Void

    var body: some View {
        if let standing = mirror.connection?.standing, standing != .fine {
            HStack(spacing: 12) {
                KoanLabel(
                    Self.title(standing),
                    icon: standing == .unreachable ? Icon.offline : Icon.warning
                )
                .foregroundStyle(KoanTheme.style(.bad, system: .orange))
                .lineLimit(2)
                Spacer(minLength: 0)
                if standing != .unreachable {
                    Button("Sign In", action: signIn)
                }
            }
            .font(.role(.fine, system: .callout))
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity)
            #if os(tvOS)
            // tvOS has no bar material.
            .koanMaterial(.regularMaterial)
            #else
            .koanMaterial(.bar)
            #endif
            .transition(.opacity)
        }
    }

    private static func title(_ standing: ServerStanding) -> String {
        switch standing {
        case .signedOut: "Signed out of your server"
        case .refused: "Your server refused kōan's sign-in"
        case .unreachable: "Can't reach your server. kōan keeps trying."
        case .fine: ""
        }
    }
}

/// The output this device is still waiting for, once it has taken a while
/// (`NowPlaying.outputWait`): a busy audio system or a device waking. The
/// player goes on taking commands and starts from wherever they leave it;
/// Retry asks the device again, Cancel plays on the system's default instead.
/// Reads the Playback slice itself, so its parent is not drawn again with
/// every track.
struct OutputWaitBanner: View {
    @Environment(EngineMirror.self) private var mirror

    var body: some View {
        let wait = mirror.playback.outputWait
        VStack(spacing: 0) {
            if let wait { Line(wait: wait) }
        }
        .koanAnimation(KoanTheme.Motion.normal, value: wait)
    }

    private struct Line: View {
        @Environment(PlayerModel.self) private var player
        let wait: OutputWait

        var body: some View {
                HStack(spacing: 12) {
                    KoanLabel(
                        wait.device.map { "Waiting for \($0)…" } ?? "Waiting for the system's output…",
                        icon: Icon.speaker
                    )
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                    .lineLimit(2)
                    Spacer(minLength: 0)
                    Button("Retry") { player.retryOutput() }
                    Button("Cancel") { player.cancelOutputWait() }
                }
                .font(.role(.fine, system: .callout))
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                .frame(maxWidth: .infinity)
                #if os(tvOS)
                .koanMaterial(.regularMaterial)
                #else
                .koanMaterial(.bar)
                #endif
                .transition(.opacity)
        }
    }
}
