import Combine
import KoanFFI
import SwiftUI

/// koan on iOS.
///
/// The same engine, the same models and the same pages as the Mac app — what
/// differs is the shell around them. A phone has no menu bar, no sidebar and no
/// second window, so the navigator is driven by a tab bar and the transport
/// sits above it rather than across the top.
@main
struct KoanIOSApp: App {
    @UIApplicationDelegateAdaptor(PushDelegate.self) private var push
    @State private var state: AppState?
    @State private var startupError: String?
    @State private var session = AudioSession()
    @State private var keepalive = Keepalive()
    @Environment(\.scenePhase) private var scenePhase
    @State private var powerSaving = ProcessInfo.processInfo.isLowPowerModeEnabled

    var body: some Scene {
        WindowGroup {
            Group {
                if let state {
                    TabShell()
                        .environment(state)
                        .environment(state.player)
                        .environment(state.library)
                        .environment(state.nav)
                        .environment(state.search)
                        .environment(state.art)
                        .environment(state.organize)
                        .environment(state.playlists)
                        .environment(state.activity)
                        .environment(state.levels)
                        .environment(state.ui)
                        .environment(state.mirror)
                        .environment(\.powerSaving, powerSaving)
                        .tint(.koanAccent)
                } else if let startupError {
                    ContentUnavailableView(
                        "kōan could not start",
                        systemImage: "exclamationmark.triangle",
                        description: Text(startupError)
                    )
                } else {
                    Splash()
                }
            }
            // Posted from whichever thread noticed; read again rather than
            // trusting the notification to say which way it went.
            .onReceive(
                NotificationCenter.default
                    .publisher(for: Notification.Name.NSProcessInfoPowerStateDidChange)
                    .receive(on: RunLoop.main)
            ) { _ in
                powerSaving = ProcessInfo.processInfo.isLowPowerModeEnabled
            }
            .onChange(of: scenePhase) { _, phase in
                keepalive.setBackground(phase == .background)
                state?.player.engine.logNote(message: "scene \(phase)")
                // Suspended in the background, the link to the server went
                // with the rest of the app; link again now rather than when
                // its retry comes round.
                if phase == .active { state?.player.engine.linkNudge() }
            }
            .onChange(of: state?.player.isPlaying ?? false) { _, playing in
                keepalive.setPlaying(playing)
            }
            .task {
                guard state == nil, startupError == nil else { return }
                do {
                    let built = try await AppState()
                    await built.start()
                    PushDelegate.engine = built.player.engine
                    PushDelegate.requestAlertsIfSignedIn()
                    // The session goes up before anything can be asked to play:
                    // a RemoteIO unit on an inactive session produces silence
                    // and reports success, which is the worst of both.
                    session.activate()
                    // Whether the music was playing when the interruption
                    // began, which the pause below makes unreadable after.
                    var interruptedPlaying = false
                    session.onInterrupted = { [weak built] in
                        interruptedPlaying = built?.player.isPlaying ?? false
                        built?.player.engine.logNote(message: "interrupted while playing: \(interruptedPlaying)")
                        built?.player.pause()
                    }
                    session.onRouteLost = { [weak built] in built?.player.pause() }
                    session.note = { [weak built] message in built?.player.engine.logNote(message: message) }
                    session.onInterruptionEnded = { [weak built] shouldResume in
                        guard let engine = built?.player.engine else { return }
                        let resume = shouldResume && interruptedPlaying
                        engine.logNote(message: "interruption over: resume=\(resume) (system says \(shouldResume), was playing \(interruptedPlaying))")
                        interruptedPlaying = false
                        Task {
                            try? await engine.restartOutput()
                            if resume { try? await engine.resume() }
                        }
                    }
                    state = built
                } catch {
                    startupError = String(describing: error)
                }
            }
        }
    }
}

/// What shows while the engine starts: the launch screen, continued.
///
/// The system draws `LaunchEnso` on `LaunchBackground` before any of koan has
/// run; this draws the same image at the same size in the same place, so the
/// moment the app takes over is not a moment anyone sees.
private struct Splash: View {
    var body: some View {
        Image("LaunchEnso")
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color("LaunchBackground"))
            .ignoresSafeArea()
    }
}
