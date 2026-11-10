import ActivityKit
import KoanFFI
import UIKit

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
    private var activity: Activity<RemoteActivity>?
    private var tokens: Task<Void, Never>?
    private var shown: RemoteActivity.ContentState?
    /// The sleeve as the activity carries it, and which record it is of.
    private var sleeve: (album: Int64, data: Data?)?
    private var devices: [DeviceInfo] = []
    private var devicesAt = Date.now
    private var target: String?

    init(engine: KoanEngine) {
        self.engine = engine
        Task {
            await RemoteActivityCommands.shared.set { device, command in
                try await engine.commandDevice(id: device, command: command)
            }
        }
        if let left = Activity<RemoteActivity>.activities.first {
            activity = left
            follow(left)
        }
        let stream = engine.observeDevices()
        Task { [weak self] in
            while let batch = await stream.next() {
                guard let self else { return }
                for case .devices(let devices, let target, _) in batch {
                    self.devices = devices
                    self.devicesAt = .now
                    self.target = target
                    self.refresh()
                }
            }
        }
    }

    private func refresh() {
        guard let target, let device = devices.first(where: { $0.id == target }) else {
            if target == nil { end() }
            return
        }
        let state = RemoteActivity.ContentState(
            device: device.name,
            linked: device.awake,
            title: device.title,
            artist: device.artistCredit.isEmpty ? nil : device.artistCredit.map(\.text).joined(),
            album: device.album,
            playing: device.state == .playing,
            positionMs: device.positionMs,
            durationMs: device.durationMs,
            at: devicesAt.timeIntervalSince1970,
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
        sleeve = (album, nil)
        let engine = engine
        Task {
            guard let art = try? await engine.albumCoverArt(albumId: album, size: 144),
                  let small = await Task.detached(operation: { Self.jpeg(art.data) }).value,
                  sleeve?.album == album
            else { return }
            sleeve = (album, small)
            refresh()
        }
        return nil
    }

    /// Small enough for the 4 KB Apple allows an activity's state.
    private nonisolated static func jpeg(_ data: Data) -> Data? {
        guard let image = UIImage(data: data) else { return nil }
        let side: CGFloat = 96
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: side, height: side), format: format)
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
