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
    weak var activity: ActivityModel?

    /// What the sheet is working on. Non-nil means the sheet is up.
    private(set) var subject: Subject?

    /// Named patterns from the config, plus a free-text one for a pattern you
    /// only want this once.
    private(set) var patterns: [OrganizePattern] = []
    private(set) var folders: [String] = []

    /// The chosen pattern's name.
    var patternName: String? {
        didSet {
            guard !configuring else { return }
            editing = false
            loadSelectedPattern()
        }
    }
    /// The format string being edited. Previews as you type; only written back
    /// to the config when saved, so trying one out costs nothing.
    var draft = "" { didSet { schedulePreview() } }
    private(set) var editing = false
    /// Which library folder the pattern's relative paths hang off.
    var baseDir: String = "" { didSet { schedulePreview() } }

    private(set) var plan: OrganizePlan?
    private(set) var previewing = false
    private(set) var running = false
    private(set) var error: String?

    private var previewTask: Task<Void, Never>?
    /// The selection, read once when the sheet opens. Patterns are generated
    /// against it — no database, no filesystem — which is what makes typing
    /// feel like typing.
    private var selection: OrganizeSelection?
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
        error = nil
        editing = false
        patterns = engine.organizePatterns()
        folders = engine.libraryFolders()
        baseDir = folders.first ?? ""
        patternName = patterns.first(where: \.isDefault)?.name ?? patterns.first?.name
        draft = stored(patternName) ?? ""
        configuring = false
        resolveSelection(trackIds: trackIds)
    }

    func dismiss() {
        previewTask?.cancel()
        previewTask = nil
        subject = nil
        selection = nil
        plan = nil
    }

    /// Read the selection once. Everything after this is generating patterns
    /// against what it returns.
    private func resolveSelection(trackIds: [Int64]) {
        let engine = self.engine
        generation += 1
        let requested = generation
        previewing = true
        Task {
            let resolved = await Task.detached(priority: .userInitiated) {
                try? engine.organizeSelection(trackIds: trackIds)
            }.value
            guard requested == generation, subject != nil else { return }
            selection = resolved
            previewing = false
            guard resolved != nil else {
                error = "Could not read those tracks."
                return
            }
            refreshPreview()
        }
    }

    // MARK: - The pattern

    /// The format string the preview and the run both use. Always the draft:
    /// picking a pattern loads it into the draft, so there is one answer to
    /// "what is about to happen" rather than two that can disagree.
    var pattern: String { draft }

    /// What the config holds for a name, before any editing.
    private func stored(_ name: String?) -> String? {
        guard let name else { return nil }
        return patterns.first { $0.name == name }?.pattern
    }

    /// True once the draft differs from what is stored — the only state in
    /// which saving does anything.
    var isModified: Bool { draft != stored(patternName) }

    func beginEditing() { editing = true }

    /// Put the stored pattern back and stop editing. The preview follows,
    /// because it reads the draft.
    func cancelEditing() {
        editing = false
        draft = stored(patternName) ?? draft
    }

    /// Write the draft to `config.toml` under the selected name.
    func saveEditing() {
        guard let name = patternName, isModified else {
            editing = false
            return
        }
        do {
            try engine.saveOrganizePattern(name: name, pattern: draft)
            patterns = engine.organizePatterns()
            editing = false
            error = nil
        } catch {
            self.error = String(describing: error)
        }
    }

    /// Selecting a different pattern loads it, abandoning an unsaved draft —
    /// which is what picking a different one means.
    private func loadSelectedPattern() {
        guard let replacement = stored(patternName) else { return }
        draft = replacement
    }

    // MARK: - Preview

    /// Only the disk pass is worth debouncing, and `refreshPreview` puts the
    /// pure result up before waiting on it — so the delay is invisible on the
    /// destinations and only defers the conflict flags.
    private func schedulePreview() {
        previewTask?.cancel()
        guard subject != nil, selection != nil, !configuring else { return }
        previewTask = Task {
            guard !Task.isCancelled else { return }
            refreshPreview()
        }
    }

    /// Two passes.
    ///
    /// The first is pure — the pattern formatted into destinations, no files
    /// touched — and lands immediately, so the table tracks what is being
    /// typed. The second asks the disk which destinations are already occupied
    /// and what artwork travels along, and swaps in when it returns. The fast
    /// answer is never wrong, only less complete, so there is nothing to
    /// unsee when the slow one arrives.
    func refreshPreview() {
        guard let selection, !pattern.isEmpty else {
            plan = nil
            return
        }
        let (pattern, base) = (self.pattern, self.baseDir)

        generation += 1
        let requested = generation

        plan = selection.generate(pattern: pattern, baseDir: base)
        error = nil

        previewing = true
        Task {
            let checked = await Task.detached(priority: .userInitiated) {
                selection.check(pattern: pattern, baseDir: base)
            }.value
            guard requested == generation else { return }
            previewing = false
            plan = checked
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
            // Exclusive: the moves and the row rewrites share a transaction, so
            // this contends for the writer the way a scan does.
            let result = await activity?.run("Moving files", exclusive: true) {
                try engine.organizeExecute(pattern: pattern, trackIds: ids, baseDir: base)
            } ?? .failure(OrganizeFailure.noEngine)
            guard requested == generation else { return }
            running = false
            switch result {
            case .success:
                // Re-plan against where the files now are, rather than leaving
                // the table showing moves that have already happened. Every
                // file that made it comes back as unchanged — a column of
                // ticks — and anything blocked is still blocked, for the same
                // reason it was. The table stays the report.
                error = nil
                refreshPreview()
            case .failure(let failure):
                previewing = false
                self.plan = nil
                error = String(describing: failure)
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

    /// `activity` is wired at startup and is the only route to the engine for
    /// a run, so its absence is a programming error rather than a state the
    /// sheet has to render.
    enum OrganizeFailure: Error {
        case noEngine
    }

}
