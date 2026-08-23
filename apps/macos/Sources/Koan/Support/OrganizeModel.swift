import Foundation
import KoanFFI

/// State for the organize sheet: which pattern, which library folder, and the
/// plan those two produce.
///
/// The plan is the product here, not a step on the way to one. Music files are
/// irreplaceable, so nothing moves until the user has seen every destination —
/// including the ones that are blocked, which is the half a preview usually
/// leaves out.
@MainActor
@Observable
final class OrganizeModel {
    private let engine: KoanEngine

    /// What the sheet is working on. Non-nil means the sheet is up.
    private(set) var subject: Subject?

    /// Named patterns from the config, plus a free-text one for a pattern you
    /// only want this once.
    private(set) var patterns: [OrganizePattern] = []
    private(set) var folders: [String] = []

    /// The chosen pattern's name, or nil while editing a custom one.
    var patternName: String? { didSet { schedulePreview() } }
    var customPattern = "" { didSet { schedulePreview() } }
    /// Which library folder the pattern's relative paths hang off.
    var baseDir: String = "" { didSet { schedulePreview() } }

    private(set) var plan: OrganizePlan?
    private(set) var previewing = false
    private(set) var running = false
    /// Set once a run finishes, so the sheet can report instead of re-arming.
    private(set) var outcome: String?
    private(set) var error: String?

    private var previewTask: Task<Void, Never>?
    /// Bumped per request, so a slow plan can't land on top of a newer one. The
    /// custom field previews as it's typed, so overlapping requests are normal.
    private var generation = 0
    /// Set while `begin` fills the controls in, so seeding them doesn't queue a
    /// preview per assignment.
    private var configuring = false

    init(engine: KoanEngine) {
        self.engine = engine
    }

    /// What a run covers. Track IDs rather than paths: everything reachable
    /// from the UI has a library row by the time it gets here, dropped files
    /// included, and a row carries the album's date and label — facts the file's
    /// own tags may not.
    struct Subject {
        let title: String
        let trackIds: [Int64]
    }

    // MARK: - Presenting

    func begin(title: String, trackIds: [Int64]) {
        configuring = true
        subject = Subject(title: title, trackIds: trackIds)
        plan = nil
        outcome = nil
        error = nil
        patterns = engine.organizePatterns()
        folders = engine.libraryFolders()
        baseDir = folders.first ?? ""
        patternName = patterns.first(where: \.isDefault)?.name ?? patterns.first?.name
        customPattern = pattern
        configuring = false
        refreshPreview()
    }

    func dismiss() {
        previewTask?.cancel()
        previewTask = nil
        subject = nil
        plan = nil
    }

    // MARK: - The pattern

    /// The format string in effect: a named pattern if one is chosen, the
    /// free-text field otherwise.
    var pattern: String {
        guard let patternName else { return customPattern }
        return patterns.first { $0.name == patternName }?.pattern ?? customPattern
    }

    var isCustom: Bool { patternName == nil }

    /// Switch to editing the pattern by hand, seeded with whatever is selected
    /// — nobody writes one of these from a blank field.
    func startCustomPattern() {
        customPattern = pattern
        patternName = nil
    }

    // MARK: - Preview

    /// Debounced, because the custom field previews as it is typed and each
    /// pass resolves a destination for every selected file.
    private func schedulePreview() {
        previewTask?.cancel()
        guard subject != nil, !configuring else { return }
        // Changing anything puts the sheet back into preview mode, so a finished
        // run reports what it did without stranding you on the report.
        outcome = nil
        previewTask = Task {
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            refreshPreview()
        }
    }

    func refreshPreview() {
        guard let subject, !pattern.isEmpty else {
            plan = nil
            return
        }
        let engine = self.engine
        let (pattern, base) = (self.pattern, self.baseDir)
        let ids = subject.trackIds

        generation += 1
        let requested = generation
        previewing = true
        Task {
            let result = await Task.detached(priority: .userInitiated) {
                Result { try engine.organizePreview(pattern: pattern, trackIds: ids, baseDir: base) }
            }.value
            // A later keystroke already asked for a newer plan, or the sheet is gone.
            guard requested == generation, self.subject != nil else { return }
            previewing = false
            apply(result)
        }
    }

    // MARK: - Running

    func run() {
        guard let subject, let plan, plan.movedCount > 0, !running else { return }
        let engine = self.engine
        let (pattern, base) = (self.pattern, self.baseDir)
        let ids = subject.trackIds

        previewTask?.cancel()
        // Claim the generation so an in-flight preview can't overwrite the
        // result of the run with a plan made before it.
        generation += 1
        let requested = generation
        running = true
        error = nil
        Task {
            let result = await Task.detached(priority: .userInitiated) {
                Result { try engine.organizeExecute(pattern: pattern, trackIds: ids, baseDir: base) }
            }.value
            guard requested == generation else { return }
            running = false
            previewing = false
            apply(result)
            if case .success(let done) = result {
                outcome = Self.summary(of: done)
            }
        }
    }

    private func apply(_ result: Result<OrganizePlan, Error>) {
        switch result {
        case .success(let plan):
            self.plan = plan
            error = nil
        case .failure(let failure):
            plan = nil
            error = String(describing: failure)
        }
    }

    /// What a finished run did, in the order it matters: what moved, then what
    /// didn't and why.
    private static func summary(of plan: OrganizePlan) -> String {
        var parts = [Format.count(Int64(plan.movedCount), "file") + " moved"]
        if plan.conflictCount > 0 {
            parts.append("\(plan.conflictCount) blocked")
        }
        if plan.errorCount > 0 {
            parts.append("\(plan.errorCount) failed")
        }
        return parts.joined(separator: " · ")
    }
}
