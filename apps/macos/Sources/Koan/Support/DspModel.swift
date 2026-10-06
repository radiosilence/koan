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
    /// The port iOS is routing audio to, which profiles are chosen by on a
    /// phone. Set on each route change; nil on the Mac.
    private(set) var route: String?
    var lastError: String?
    /// An import waiting on the rate of what it was given.
    var needsRate: Pending?
    /// AutoEQ's results for the last search.
    private(set) var autoEqResults: [AutoEqEntry] = []
    /// The makers in AutoEQ's index, to browse when nothing is typed.
    private(set) var autoEqMakers: [AutoEqMaker] = []
    /// Moves with every search, so one that returns after a newer one began
    /// is dropped rather than shown for the wrong query.
    private var autoEqSearch = 0
    /// AutoEQ's profile for the output in use, by its name, while the output
    /// has none and the offer has not been turned down.
    private(set) var suggestion: AutoEqOffer?

    enum Pending {
        case files([URL], name: String?)
        case text(String)
    }

    init(engine: KoanEngine) {
        self.engine = engine
    }

    func reload() {
        Task {
            overview = await engine.dspOverview()
            suggestion = await engine.autoeqSuggestion()
        }
    }

    /// The route changed: what the Now Playing preset names and assigns to
    /// follows it. The engine has been told already.
    func follow(route: String) {
        self.route = route
        reload()
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

    // MARK: - AutoEQ

    /// Search AutoEQ by headphone name. The view debounces; this runs once
    /// per settled query.
    func searchAutoEq(_ query: String) async {
        autoEqSearch += 1
        let search = autoEqSearch
        let trimmed = query.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else {
            autoEqResults = []
            return
        }
        do {
            let found = try await engine.autoeqSearch(query: trimmed, limit: 40)
            guard search == autoEqSearch else { return }
            autoEqResults = found
        } catch {
            guard search == autoEqSearch else { return }
            lastError = SettingsModel.describe(error)
        }
    }

    /// The makers to browse, read once and kept for as long as the model:
    /// the index changes daily at most.
    func loadAutoEqMakers() async {
        guard autoEqMakers.isEmpty else { return }
        do {
            autoEqMakers = try await engine.autoeqMakers()
        } catch {
            lastError = SettingsModel.describe(error)
        }
    }

    /// `maker`'s results, by model.
    func autoEqModels(_ maker: String) async -> [AutoEqEntry] {
        do {
            return try await engine.autoeqModels(maker: maker)
        } catch {
            lastError = SettingsModel.describe(error)
            return []
        }
    }

    /// Install `entry` as a profile and play the output in use through it.
    func installAutoEq(_ entry: AutoEqEntry) {
        let device = overview?.device
        act {
            _ = try await $0.autoeqInstall(
                name: entry.name, measuredBy: entry.measuredBy, device: device
            )
        }
        suggestion = nil
    }

    /// Stop offering AutoEQ's profile for the output in use.
    func dismissSuggestion() {
        suggestion = nil
        act { try await $0.autoeqDismiss() }
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

    /// The targets `name`'s correction can be moved to.
    func targets(_ name: String) async -> DspTargets? {
        await engine.dspTargets(name: name)
    }

    /// Move `name`'s correction to `id`, or with nil back to its own target.
    func chooseTarget(_ name: String, _ id: String?) {
        act { try await $0.dspChooseTarget(name: name, id: id) }
    }

    /// Add a target from a file, picked or shared: security-scoped, readable
    /// only while held open.
    func addTarget(_ url: URL) async {
        let held = url.startAccessingSecurityScopedResource()
        defer { if held { url.stopAccessingSecurityScopedResource() } }
        do {
            _ = try await engine.dspAddTarget(path: url.path)
            lastError = nil
            await changed()
        } catch {
            lastError = SettingsModel.describe(error)
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
