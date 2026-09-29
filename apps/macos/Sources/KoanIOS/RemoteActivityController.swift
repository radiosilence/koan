import ActivityKit
import KoanFFI
import SwiftUI

/// Starts, updates and ends the lock screen's Live Activity as this phone
/// starts and stops controlling another device. See `RemoteActivity`.
///
/// Updated from here while the app runs, and by the server's pushes once iOS
/// has suspended it; the push token goes up the link for that.
@MainActor
final class RemoteActivityController {
    private let engine: KoanEngine
    private let mirror: EngineMirror
    private var activity: Activity<RemoteActivity>?
    private var tokens: Task<Void, Never>?
    private var shown: RemoteActivity.ContentState?

    init(engine: KoanEngine, mirror: EngineMirror) {
        self.engine = engine
        self.mirror = mirror
        Task {
            await RemoteActivityCommands.shared.set { device, command in
                try await engine.commandDevice(id: device, command: command)
            }
        }
        // An activity outlives the process. Controlling its device again is
        // what the person left the phone doing.
        if let left = Activity<RemoteActivity>.activities.first {
            activity = left
            follow(left)
            Task { try? await engine.controlDevice(id: left.attributes.deviceId) }
        }
        mirror.follow { [weak self] in self?.refresh() }
    }

    private func refresh() {
        guard let target = mirror.target,
              let device = mirror.devices.first(where: { $0.id == target })
        else {
            // The target may be listed again in a moment (a link reconnecting);
            // only a return to this device ends the activity.
            if mirror.target == nil { end() }
            return
        }
        let state = RemoteActivity.ContentState(
            device: device.name,
            linked: device.awake,
            title: device.title,
            artist: device.artist,
            album: device.album,
            playing: device.state == .playing,
            positionMs: device.positionMs,
            durationMs: device.durationMs,
            at: mirror.devicesAt.timeIntervalSince1970
        )
        if let activity, activity.attributes.deviceId == target {
            guard state != shown else { return }
            shown = state
            Task { await activity.update(ActivityContent(state: state, staleDate: nil)) }
        } else {
            end()
            start(target, state)
        }
    }

    private func start(_ target: String, _ state: RemoteActivity.ContentState) {
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }
        do {
            let started = try Activity.request(
                attributes: RemoteActivity(deviceId: target),
                content: ActivityContent(state: state, staleDate: nil),
                pushType: .token
            )
            activity = started
            shown = state
            follow(started)
        } catch {
            engine.logNote(message: "live activity refused: \(error)")
        }
    }

    /// Hand the activity's push tokens to the link as iOS issues them.
    private func follow(_ activity: Activity<RemoteActivity>) {
        tokens?.cancel()
        let (engine, device) = (engine, activity.attributes.deviceId)
        tokens = Task {
            for await data in activity.pushTokenUpdates {
                let token = data.map { String(format: "%02x", $0) }.joined()
                engine.setLiveActivity(token: token, device: device, sandbox: PushDelegate.sandbox)
            }
        }
    }

    private func end() {
        tokens?.cancel()
        tokens = nil
        guard let ending = activity else { return }
        activity = nil
        shown = nil
        engine.setLiveActivity(token: nil, device: nil, sandbox: false)
        Task { await ending.end(nil, dismissalPolicy: .immediate) }
    }
}
