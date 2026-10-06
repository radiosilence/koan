import Combine
import KoanFFI
import SwiftUI

/// koan on iOS.
///
/// The same engine, the same models and the same pages as the Mac app — what
/// differs is the shell around them. A phone has no menu bar, no sidebar and no
/// second window, so the navigator is driven by a tab bar and the transport
/// sits above it.
@main
struct KoanIOSApp: App {
    @UIApplicationDelegateAdaptor(PushDelegate.self) private var push
    @State private var state: AppState?
    @State private var startupError: String?
    /// A link opened before the engine was up, handled once it is.
    @State private var pendingURL: URL?
    @State private var session = AudioSession()
    #if !os(tvOS)
    @State private var remoteActivity: RemoteActivityController?
    #endif
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
                        .environment(state.meter)
                        .environment(state.ui)
                        .environment(state.mirror)
                        .environment(\.powerSaving, powerSaving)
                        .modifier(InviteConfirmation(state: state))
                        .modifier(PairingConfirmation(state: state))
                        .modifier(DspImportPrompts(dsp: state.dsp))
                        #if os(tvOS)
                        // No app-wide accent: a television draws focus as a
                        // white platter, and system alerts and toggle rows
                        // that take the tint put green text on it.
                        #else
                        .tint(.koanAccent)
                        #endif
                        #if os(tvOS)
                        // The wash is drawn for a dark room; a television set
                        // to light would grey it out.
                        .preferredColorScheme(.dark)
                        #endif
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
            // An invite, as a universal link or through `koan://join`; a filter
            // or EQ file opened in koan or shared to it; `koan://dsp-inbox`,
            // which the share extension opens after leaving something.
            .onOpenURL { url in
                if let state { open(url, in: state) } else { pendingURL = url }
            }
            .onContinueUserActivity(NSUserActivityTypeBrowsingWeb) { activity in
                guard let url = activity.webpageURL else { return }
                if let state { state.open(url: url) } else { pendingURL = url }
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
                state?.player.engine.setBackground(background: phase == .background)
                state?.player.engine.logNote(message: "scene \(phase)")
                if phase == .active { session.recoverIfInterrupted() }
                // Suspended in the background, the link to the server went
                // with the rest of the app; link again now rather than when
                // its retry comes round.
                if phase == .active { state?.player.engine.linkNudge() }
                if phase == .active, let state { ShareInbox.collect(into: state.dsp) }
            }
            .onChange(of: state?.player.isPlaying ?? false) { _, playing in
                state?.player.engine.setPlaying(playing: playing)
            }
            .task {
                guard state == nil, startupError == nil else { return }
                do {
                    let built = try await AppState()
                    await built.start()
                    PushDelegate.engine = built.player.engine
                    #if !os(tvOS)
                    remoteActivity = RemoteActivityController(engine: built.player.engine, mirror: built.mirror, art: built.art)
                    #endif
                    PushDelegate.requestAlertsIfSignedIn()
                    // The session goes up before anything can be asked to play:
                    // a RemoteIO unit on an inactive session produces silence
                    // and reports success, which is the worst of both.
                    session.activate()
                    // Whether the music was playing when the interruption
                    // began, which the pause below makes unreadable after.
                    var interruptedPlaying = false
                    session.onInterrupted = { [weak built] in
                        // iOS can send several "began" for one interruption.
                        // The later ones find playback already paused (by the
                        // first), so they must not overwrite what the first saw,
                        // or the "resume" at the end finds nothing to resume.
                        interruptedPlaying = interruptedPlaying || (built?.player.engine.isPlaying() ?? false)
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
                    if let pendingURL {
                        open(pendingURL, in: built)
                        self.pendingURL = nil
                    }
                    session.onRoute = { [weak built] name in
                        guard let built else { return }
                        let engine = built.player.engine
                        Task {
                            try? await engine.setAudioRoute(name: name)
                            built.dsp.follow(route: name)
                        }
                    }
                    ShareInbox.collect(into: built.dsp)
                } catch {
                    startupError = String(describing: error)
                }
            }
        }
    }
}

extension KoanIOSApp {
    private func open(_ url: URL, in state: AppState) {
        if url.isFileURL {
            state.dsp.importFiles([url])
        } else if url.scheme == "koan", url.host == "dsp-inbox" {
            ShareInbox.collect(into: state.dsp)
        } else {
            state.open(url: url)
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
            // After the launch screen, which is drawn before any code runs and
            // so cannot say it: the ensō stays put, and this arrives under it.
            .overlay(alignment: .bottom) {
                Text(AppVersion.text)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.bottom, 24)
            }
    }
}
