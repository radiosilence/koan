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
                        .koanTheme(state.appearance)
                        .environment(\.powerSaving, powerSaving)
                        .modifier(InviteConfirmation(state: state))
                        .modifier(PairingConfirmation(state: state))
                        .modifier(DspImportPrompts(dsp: state.dsp))
                        #if !os(tvOS)
                        .modifier(TvWakePrompts(state: state))
                        // In front only: a profile read after a background
                        // wake would ask where iOS cannot show the question.
                        .onChange(
                            of: scenePhase == .active && state.mirror.offers(PushDelegate.extensionName),
                            initial: true
                        ) { _, ask in
                            if ask { Task { await PushDelegate.requestAlerts() } }
                        }
                        #endif
                        // No app-wide accent on a television: it draws focus
                        // as a white platter, and system alerts and toggle
                        // rows that take the tint put green text on it.
                        #if !os(tvOS)
                        .tint(.koanAccent)
                        #endif
                        #if os(tvOS)
                        // The wash is drawn for a dark room; a television set
                        // to light would grey it out.
                        .preferredColorScheme(.dark)
                        #endif
                } else if let startupError {
                    KoanUnavailable("kōan could not start", icon: Icon.warning, detail: startupError)
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
            .task {
                guard state == nil, startupError == nil else { return }
                do {
                    let built = try await AppState()
                    await built.start()
                    PushDelegate.engine = built.player.engine
                    #if !os(tvOS)
                    remoteActivity = RemoteActivityController(engine: built.player.engine, mirror: built.mirror, art: built.art)
                    #endif
                    // Configured before anything can be asked to play, and
                    // activated by the engine only when something does: koan
                    // opening must not stop another app's music.
                    session.prepare()
                    let engine = built.player.engine
                    engine.setAudioSession(host: AudioSession.Host { message in
                        engine.logNote(message: message)
                    })
                    // Whether the music was playing when the interruption
                    // began, which the pause below makes unreadable after.
                    var interruptedPlaying = false
                    // The phone's own audio events are about the phone. While
                    // another device is controlled this phone plays nothing, and
                    // a pause or resume sent from here would land on that device;
                    // while a renderer plays, the phone's output is not what is
                    // heard, and a call or unplugged headphones change nothing.
                    session.onInterrupted = { [weak built] in
                        guard built?.player.playsHere == true else { return }
                        // iOS can send several "began" for one interruption.
                        // The later ones find playback already paused (by the
                        // first), so they must not overwrite what the first saw,
                        // or the "resume" at the end finds nothing to resume.
                        interruptedPlaying = interruptedPlaying || (built?.player.engine.isPlaying() ?? false)
                        built?.player.engine.logNote(message: "interrupted while playing: \(interruptedPlaying)")
                        built?.player.pause()
                    }
                    session.onRouteLost = { [weak built] in
                        guard built?.player.playsHere == true else { return }
                        built?.player.pause()
                    }
                    session.note = { [weak built] message in built?.player.engine.logNote(message: message) }
                    session.onInterruptionEnded = { [weak built] shouldResume in
                        guard let built else { return }
                        let engine = built.player.engine
                        let resume = shouldResume && interruptedPlaying && built.player.playsHere
                        engine.logNote(message: "interruption over: resume=\(resume) (system says \(shouldResume), was playing \(interruptedPlaying))")
                        interruptedPlaying = false
                        // The unit iOS stopped does not start again, so it is
                        // made afresh whenever the phone's own output is the
                        // output, playing here or not. A renderer's session is
                        // not the phone's to restart.
                        guard built.player.renderer == nil else { return }
                        Task {
                            try? await engine.restartOutput()
                            if resume { try? await engine.resume() }
                        }
                    }
                    state = built
                    // `onChange(of: scenePhase)` reports changes only, and none
                    // while the engine was being built. A launch into the
                    // background, as for a push after iOS ended the app, has
                    // none at all, and would otherwise run as if in front.
                    engine.setBackground(background: UIApplication.shared.applicationState == .background)
                    // Followed on the mirror rather than through a view, whose
                    // updates are not promised while the app is in the background.
                    var playing: Bool?
                    built.mirror.follow { [weak built] in
                        guard let now = built?.player.isPlaying, now != playing else { return }
                        playing = now
                        engine.setPlaying(playing: now)
                    }
                    #if os(tvOS)
                    IntentTarget.set(built)
                    TopShelfWriter.start(built)
                    #endif
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
                    ShareInbox.sweepOpened()
                    ShareInbox.collect(into: built.dsp)
                } catch {
                    startupError = String(describing: error)
                    #if os(tvOS)
                    IntentTarget.fail(String(describing: error))
                    #endif
                }
            }
        }
    }
}

extension KoanIOSApp {
    private func open(_ url: URL, in state: AppState) {
        if url.isFileURL {
            // A copy iOS made in the app's Inbox for it, not opened in place.
            state.dsp.importFiles([url], copy: url)
        } else if url.scheme == "koan", url.host == "dsp-inbox" {
            ShareInbox.collect(into: state.dsp)
        #if os(tvOS)
        } else if url.scheme == "koan", url.host == "album" {
            openRecord(url, in: state)
        #endif
        } else {
            state.open(url: url)
        }
    }
}

#if os(tvOS)
extension KoanIOSApp {
    /// A record chosen on the Top Shelf: `koan://album/<uid>` opens its page,
    /// `?play` plays it. Television only: on a phone or a Mac the scheme would
    /// let any web page start the music.
    private func openRecord(_ url: URL, in state: AppState) {
        let uid = url.lastPathComponent
        let play = URLComponents(url: url, resolvingAgainstBaseURL: false)?
            .queryItems?.contains { $0.name == "play" } == true
        Task {
            let found: Int64?
            do {
                found = try await state.engine.albumForUid(uid: uid)
            } catch {
                state.player.lastError = SettingsModel.describe(error)
                return
            }
            guard let id = found else {
                state.player.lastNotice = "That record is no longer in the library."
                return
            }
            if play {
                do {
                    _ = try await state.playRecord(id, named: "That record")
                    state.ui.nowPlayingAsked += 1
                } catch let error as IntentError {
                    state.player.lastError = String(localized: error.localizedStringResource)
                } catch {
                    state.player.lastError = SettingsModel.describe(error)
                }
            } else {
                state.nav.open(album: id)
            }
        }
    }
}
#endif

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
                    .font(.role(.fine, system: .caption))
                    .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                    .padding(.bottom, 24)
            }
    }
}
