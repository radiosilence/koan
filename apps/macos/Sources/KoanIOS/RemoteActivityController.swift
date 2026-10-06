import ActivityKit
import KoanFFI
import SwiftUI

/// Starts, updates and ends the lock screen's Live Activity as this phone
/// starts and stops controlling another device. See `RemoteActivity`.
///
/// Updated from here while the app runs, and by the server's pushes once iOS
/// has suspended it; the push token goes up the link for that.
///
/// It lasts exactly as long as the phone controls another device, which the
/// engine keeps on disk: an app iOS suspended or killed comes back controlling
/// the same device, and takes up the activity it left rather than starting a
/// second one.
@MainActor
final class RemoteActivityController {
    private let engine: KoanEngine
    private let mirror: EngineMirror
    private let art: CoverArtCache
    private var activity: Activity<RemoteActivity>?
    private var tokens: Task<Void, Never>?
    private var shown: RemoteActivity.ContentState?
    /// The sleeve as the activity carries it, and which record it is of.
    private var sleeve: (album: Int64, data: Data?)?

    init(engine: KoanEngine, mirror: EngineMirror, art: CoverArtCache) {
        self.engine = engine
        self.mirror = mirror
        self.art = art
        Task {
            await RemoteActivityCommands.shared.set { device, command in
                try await engine.commandDevice(id: device, command: command)
            }
        }
        if let left = Activity<RemoteActivity>.activities.first {
            activity = left
            follow(left)
        }
        mirror.follow { [weak self] in self?.refresh() }
    }

    private func refresh() {
        guard let target = mirror.target,
              let device = mirror.devices.first(where: { $0.id == target })
        else {
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
            at: mirror.devicesAt.timeIntervalSince1970,
            art: thumbnail(for: device.albumId)
        )
        if let activity, activity.attributes.deviceId == target {
            guard state != shown else { return }
            shown = state
            let id = activity.id
            Task.detached { await Self.running(id)?.update(ActivityContent(state: state, staleDate: nil)) }
        } else {
            end()
            start(target, state)
        }
    }

    /// The record's sleeve at a size the activity can carry, once it has been
    /// fetched. The fetch itself runs off to one side and refreshes when done.
    /// Each record is fetched once: one without art, or whose fetch failed,
    /// stays without a sleeve rather than being asked for again.
    private func thumbnail(for album: Int64?) -> Data? {
        guard let album else { return nil }
        if let sleeve, sleeve.album == album { return sleeve.data }
        if let image = art.cached(.album(album), size: .thumb) {
            let data = Self.jpeg(image)
            sleeve = (album, data)
            return data
        }
        sleeve = (album, nil)
        Task {
            guard let image = await art.image(for: .album(album), size: .thumb),
                  sleeve?.album == album
            else { return }
            sleeve = (album, Self.jpeg(image))
            refresh()
        }
        return nil
    }

    /// Small enough for the 4 KB Apple allows an activity's state.
    private static func jpeg(_ image: UIImage) -> Data? {
        let side: CGFloat = 72
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: side, height: side))
        let small = renderer.image { _ in
            image.draw(in: CGRect(x: 0, y: 0, width: side, height: side))
        }
        return [0.6, 0.45, 0.3].lazy
            .compactMap { small.jpegData(compressionQuality: $0) }
            .first { $0.count <= 1900 }
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
        let id = ending.id
        Task.detached { await Self.running(id)?.end(nil, dismissalPolicy: .immediate) }
    }

    /// The activity with this id, found where it is used. An `Activity` is not
    /// `Sendable`, so the one held here cannot be handed to a task; its id can.
    private nonisolated static func running(_ id: String) -> Activity<RemoteActivity>? {
        Activity<RemoteActivity>.activities.first { $0.id == id }
    }
}
