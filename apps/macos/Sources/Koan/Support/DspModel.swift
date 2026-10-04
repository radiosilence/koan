import Foundation
import KoanFFI
import Observation

/// EQ and convolution profiles, and importing them.
///
/// What Settings shows, and what a file opened in or shared to the app goes
/// through: one import flow wherever it starts, asking for a sample rate only
/// for coefficients that carry none, and offering the new profile for the
/// output in use once it is in.
@MainActor
@Observable
final class DspModel {
    private let engine: KoanEngine

    private(set) var overview: DspOverview?
    /// Moves on every change, for pages showing a profile's detail to follow.
    private(set) var version = 0
    /// The last import, to offer for the output in use.
    var imported: String?
    var lastError: String?
    /// An import waiting on the rate of what it was given.
    var needsRate: Pending?

    enum Pending {
        case files([URL], name: String?)
        case text(String)
    }

    init(engine: KoanEngine) {
        self.engine = engine
    }

    func reload() {
        Task { overview = await engine.dspOverview() }
    }

    // MARK: - Importing

    /// Files, folders or zips, as one profile. Picked or shared ones are
    /// security-scoped and readable only while held open.
    func importFiles(_ urls: [URL], name: String? = nil, rate: UInt32? = nil) {
        let engine = self.engine
        Task {
            let held = urls.filter { $0.startAccessingSecurityScopedResource() }
            defer { held.forEach { $0.stopAccessingSecurityScopedResource() } }
            await finish(.files(urls, name: name)) {
                try await engine.dspImport(paths: urls.map(\.path), name: name, rate: rate)
            }
        }
    }

    func importText(_ text: String, rate: UInt32? = nil) {
        let engine = self.engine
        Task {
            await finish(.text(text)) {
                try await engine.dspImportText(text: text, name: nil, rate: rate)
            }
        }
    }

    /// The import that stopped for a rate, with one.
    func retry(rate: UInt32) {
        guard let pending = needsRate else { return }
        needsRate = nil
        switch pending {
        case let .files(urls, name): importFiles(urls, name: name, rate: rate)
        case let .text(text): importText(text, rate: rate)
        }
    }

    private func finish(_ pending: Pending, _ run: () async throws -> String) async {
        do {
            imported = try await run()
            lastError = nil
        } catch KoanError.NeedsSampleRate {
            needsRate = pending
        } catch {
            lastError = SettingsModel.describe(error)
        }
        await changed()
    }

    private func changed() async {
        overview = await engine.dspOverview()
        version += 1
    }

    // MARK: - Choosing

    /// Play the output in use through `profile`, or untouched.
    func use(_ profile: String?) {
        act { try await $0.dspAssign(profile: profile) }
    }

    /// Play `device` through `profile`, or untouched, whether or not it is
    /// the output in use. A renderer is named by its UDN.
    func assign(_ profile: String?, to device: String) {
        act { try await $0.dspAssignDevice(device: device, profile: profile) }
    }

    /// The profile `device` plays through, if any.
    func profile(for device: String) -> String? {
        overview?.profiles.first { $0.devices.contains(device) }?.name
    }

    /// What to call a device: a renderer by its name rather than its UDN.
    func label(_ device: String) -> String {
        overview?.names[device] ?? device
    }

    func remove(_ profile: String) {
        act { try await $0.dspRemove(name: profile) }
    }

    func setEnabled(_ on: Bool) {
        act { try await $0.dspSetEnabled(enabled: on) }
    }

    private func act(_ run: @escaping @Sendable (KoanEngine) async throws -> Void) {
        let engine = self.engine
        Task {
            do {
                try await run(engine)
                lastError = nil
            } catch {
                lastError = SettingsModel.describe(error)
            }
            await changed()
        }
    }

    func detail(_ name: String) async -> DspProfileDetail? {
        await engine.dspDetail(name: name)
    }

    /// Whether it took: a name already in use is refused.
    func rename(_ old: String, to new: String) async -> Bool {
        do {
            try await engine.dspRename(old: old, new: new)
            lastError = nil
            await changed()
            return true
        } catch {
            lastError = SettingsModel.describe(error)
            return false
        }
    }

    /// Rates coefficients are commonly designed at.
    static let rates: [UInt32] = [44100, 48000, 88200, 96000, 176400, 192000]

    static func describe(_ p: DspProfileSummary) -> String {
        var parts: [String] = []
        if p.bands > 0 { parts.append("\(p.bands) \(p.bands == 1 ? "filter" : "filters")") }
        if !p.rates.isEmpty {
            parts.append(p.rates.map(khz).joined(separator: ", ") + " kHz")
        }
        return parts.joined(separator: " · ")
    }

    static func khz(_ hz: UInt32) -> String {
        let k = Double(hz) / 1000
        return k == k.rounded() ? String(format: "%.0f", k) : String(format: "%.1f", k)
    }
}
