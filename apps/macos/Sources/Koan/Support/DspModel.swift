import Foundation
import KoanFFI
import Observation

/// Files chosen together, and what importing them will do.
struct PendingImport: Identifiable {
    let id = UUID()
    let urls: [URL]
    let plan: DspImportPlan
    let rate: UInt32?
}

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
    /// What tells this model profiles changed elsewhere: synced from another
    /// device, which the engine reports as a library change.
    weak var mirror: EngineMirror?
    /// Moves on every change made here or synced from elsewhere: what a page
    /// showing profiles reloads on.
    var stamp: String { "\(version).\(mirror?.libraryVersion ?? 0)" }
    /// The last import, to offer for the output in use.
    var imported: String?
    /// The port iOS is routing audio to, which profiles are chosen by on a
    /// phone. Set on each route change; nil on the Mac.
    private(set) var route: String?
    var lastError: String?
    /// Profiles just imported, whose role is asked, once for all of them: a
    /// neutral correction, one with a tuning baked in, or a tuning on top.
    var askRole: [String]?
    /// What the last import of several files did.
    var importSummary: String?
    /// Several files planned for import, waiting to be confirmed and named.
    var pendingImport: PendingImport?
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
    /// A device whose EQ a preset menu's Edit… asked to show, on the Mac,
    /// where the EQ page is a pane of the Settings window. The page takes it
    /// and clears it.
    var editing: String?

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

    /// Files, folders or zips. Several are first planned and shown to be
    /// confirmed (`pendingImport`): whole presets become a group, parts of
    /// one profile combine into one. Picked or shared ones are
    /// security-scoped and readable only while held open.
    func importFiles(_ urls: [URL], name: String? = nil, rate: UInt32? = nil) {
        let engine = self.engine
        Task {
            if urls.count > 1, name == nil {
                let held = urls.filter { $0.startAccessingSecurityScopedResource() }
                let plan = await engine.dspImportPlan(paths: urls.map(\.path))
                held.forEach { $0.stopAccessingSecurityScopedResource() }
                pendingImport = PendingImport(urls: urls, plan: plan, rate: rate)
                return
            }
            await run(urls, name: name, rate: rate)
        }
    }

    /// Import what `pendingImport` planned, under `name`.
    func confirmImport(name: String) {
        guard let pending = pendingImport else { return }
        pendingImport = nil
        Task { await run(pending.urls, name: name, rate: pending.rate) }
    }

    private func run(_ urls: [URL], name: String?, rate: UInt32?) async {
        let held = urls.filter { $0.startAccessingSecurityScopedResource() }
        defer { held.forEach { $0.stopAccessingSecurityScopedResource() } }
        do {
            let summary = try await engine.dspImportFiles(paths: urls.map(\.path), name: name, rate: rate)
            imported = summary.group ?? summary.imported.first
            lastError = nil
            // A group's members are alike, so one answer does for them all.
            if !summary.imported.isEmpty { askRole = summary.imported }
            importSummary = Self.describe(summary, files: urls.count)
        } catch KoanError.NeedsSampleRate {
            needsRate = .files(urls, name: name)
        } catch {
            importSummary = nil
            lastError = SettingsModel.describe(error)
        }
        await changed()
    }

    /// What an import did, in a sentence or two: nothing to say for one
    /// file made into one profile.
    static func describe(_ summary: DspImportSummary, files: Int) -> String? {
        var lines: [String] = []
        if let group = summary.group {
            let playing = summary.imported.first.map { " “\($0)” is playing; pick another on the group's page or in an output's preset menu." } ?? ""
            lines.append("Imported \(summary.imported.count) presets as the group “\(group)”.\(playing)")
        } else if files > 1, let one = summary.imported.first {
            lines.append("Combined \(files) files into “\(one)”.")
        }
        if !summary.refused.isEmpty {
            let why = summary.refused.map { "\($0.file): \($0.reason)" }.joined(separator: "; ")
            lines.append("\(summary.refused.count) refused: \(why)")
        }
        lines.append(contentsOf: summary.notes)
        return lines.isEmpty ? nil : lines.joined(separator: " ")
    }

    /// Play `member` of the group `group`.
    func select(_ group: String, _ member: String) {
        act { try await $0.dspSelect(group: group, member: member) }
    }

    /// Make `name` a group, one layer playing, or a stack of layers.
    func setGroup(_ name: String, _ group: Bool) {
        act { try await $0.dspSetGroup(name: name, group: group) }
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
            askRole = imported.map { [$0] }
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

    /// The profile `device` plays through, if any: its correction.
    func profile(for device: String) -> String? {
        overview?.profiles.first { $0.devices.contains(device) }?.name
    }

    /// The tuning `device` plays on top of its correction, if any.
    func tuning(for device: String) -> String? {
        overview?.tunings[device]
    }

    /// Play `tuning` on top of `device`'s correction, or none.
    func setTuning(_ tuning: String?, for device: String) {
        act { try await $0.dspSetTuning(device: device, tuning: tuning) }
    }

    /// Make `device`'s tuning these EQs, in the order they play.
    func setTunings(_ entries: [DspTuningEntry], for device: String) {
        act { try await $0.dspSetTunings(device: device, tuning: entries) }
    }

    /// The profiles, and what `device` plays: the output in use with nil.
    func overview(for device: String?) async -> DspOverview {
        await engine.dspOverviewFor(device: device)
    }

    /// What `device` plays, drawn: its correction and tuning.
    func outputResponse(for device: String) async -> DspResponse? {
        await engine.dspOutputResponseFor(device: device, rate: 48000)
    }

    /// Set `device` from the preset `name`, or flat with nil.
    func applyPreset(_ name: String?, to device: String) {
        act { try await $0.dspApplyPreset(device: device, name: name) }
    }

    /// Save `device`'s correction and tuning as the preset `name`, over one
    /// of that name. Whether it took.
    func savePreset(_ name: String, from device: String) async -> Bool {
        do {
            _ = try await engine.dspSavePreset(device: device, name: name)
            lastError = nil
            await changed()
            return true
        } catch {
            lastError = SettingsModel.describe(error)
            return false
        }
    }

    /// The target the tuning `name` was made against, or nil for not known.
    func setTunedFor(_ name: String, _ target: String?) {
        act { try await $0.dspSetTunedFor(name: name, target: target) }
    }

    /// What to call a device: a renderer by its name rather than its UDN.
    func label(_ device: String) -> String {
        overview?.names[device] ?? device
    }

    /// Copy `name` as it is now, as `new` or "<name> copy".
    func duplicate(_ name: String, as new: String? = nil) {
        act { _ = try await $0.dspDuplicate(name: name, new: new) }
    }

    /// Put `name` back as it was imported.
    func revert(_ name: String) {
        act { try await $0.dspRevert(name: name) }
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

    /// Make `name` a stack of `layers`, in order; creates it if there is none.
    func setLayers(_ name: String, _ layers: [DspLayerInfo]) {
        act { try await $0.dspSetLayers(name: name, layers: layers) }
    }

    /// Say whether `name` corrects a headphone or tunes on top of one.
    func setRole(_ name: String, _ role: DspRole) {
        act { try await $0.dspSetRole(name: name, role: role) }
    }

    /// Say what each of `names` is for, as one import's answer.
    func setRole(_ names: [String], _ role: DspRole) {
        act { engine in
            for name in names {
                try await engine.dspSetRole(name: name, role: role)
            }
        }
    }

    /// The target a ready-made EQ was made for, or nil for Unknown.
    func setMadeFor(_ name: String, _ target: String?) {
        act { try await $0.dspSetMadeFor(name: name, target: target) }
    }

    /// The targets for in-ear or over-ear headphones, each with what it
    /// sounds like.
    func targetsFor(inEar: Bool) async -> [DspTargetOption] {
        await engine.dspTargetsFor(inEar: inEar)
    }

    /// What correcting a measurement to a target would do, before saving.
    func previewMeasurement(_ text: String, target: String) async throws -> DspResponse {
        try await engine.dspPreviewMeasurement(text: text, target: target)
    }

    /// Save a headphone's measurement corrected to a target, as a profile.
    func saveMeasured(name: String, text: String, inEar: Bool, target: String) async throws -> String {
        let saved = try await engine.dspSaveMeasured(name: name, text: text, inEar: inEar, target: target)
        imported = saved
        await changed()
        return saved
    }

    /// What splitting the baked EQ `name` would give: the correction, the
    /// tuning, their sum, and the EQ itself.
    func previewSplit(_ name: String, text: String, target: String) async throws -> DspResponse {
        try await engine.dspPreviewSplit(name: name, text: text, target: target)
    }

    /// Split the baked EQ `name` into a correction and a tuning; the outputs
    /// that played it play the two.
    func splitBaked(_ name: String, text: String, inEar: Bool, target: String) async throws -> [String] {
        let made = try await engine.dspSplitBaked(name: name, text: text, inEar: inEar, target: target)
        await changed()
        return made
    }

    /// Keep `name` on every device of the account, or on this one alone.
    func setScope(_ name: String, everywhere: Bool) {
        act { try await $0.dspSetScope(name: name, everywhere: everywhere) }
    }

    /// Set band `index` of `name`.
    func setBand(_ name: String, _ index: Int, kind: String, freq: Double, gain: Double, q: Double) {
        act {
            try await $0.dspSetBand(
                name: name, index: UInt32(index), kind: kind, freq: freq, gainDb: gain, q: q
            )
        }
    }

    func addBand(_ name: String) {
        act { _ = try await $0.dspAddBand(name: name) }
    }

    func removeFilter(_ name: String, _ index: Int) {
        act { try await $0.dspRemoveFilter(name: name, index: UInt32(index)) }
    }

    /// What `name` does to the sound, at 48 kHz, for the graph.
    func response(_ name: String) async -> DspResponse? {
        await engine.dspResponse(name: name, rate: 48000)
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
        if p.measured { parts.append("From a measurement") }
        if !p.members.isEmpty {
            parts.append("\(p.members.count) to pick from")
        } else if p.preset {
            parts.append(p.layers == 1 ? "1 part" : "\(p.layers) parts")
        } else if p.layers > 0 {
            parts.append("Plays \(p.layers) \(p.layers == 1 ? "EQ" : "EQs") in order")
        }
        if p.bands > 0 { parts.append("\(p.bands) \(p.bands == 1 ? "band" : "bands")") }
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
