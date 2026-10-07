import KoanFFI
import SwiftUI

/// A profile's graph, and for a tuning the graph as an editor: a band's
/// handle dragged for frequency and gain and pinched for Q, a graphic curve
/// painted with a brush. The graph follows the finger with a preview the
/// core draws and nothing saved, one at a time and always the newest; the
/// edit is saved, and so played, at most every quarter second while it goes
/// on, and once at its end, or when the page goes before it ends.
struct EqEditor: View {
    let dsp: DspModel
    let name: String
    let detail: DspProfileDetail
    let response: DspResponse
    var parts: [EqGraph.Part] = []

    /// The edit under way, drawn in place of `response` until a saved one is.
    @State private var live: DspResponse?
    @State private var editing = false
    /// The curve as painted so far.
    @State private var painted: [DspPoint]?
    @State private var brush = Brush.narrow
    @State private var saved = Date.distantPast
    /// The last step's save, where the throttle held it back.
    @State private var unsaved: (() -> Void)?
    /// A preview is being drawn; the newest step asked for meanwhile waits.
    @State private var drawing = false
    @State private var waiting: (() async -> DspResponse?)?

    enum Brush: Hashable { case point, narrow, wide }

    private var editable: Bool { !detail.readOnly }

    /// The graphic curve a drag away from the handles paints: the first the
    /// graph draws, on the left channel.
    private var curveIndex: Int? {
        detail.bands.firstIndex { $0.kind == "graphic" && ($0.channels.isEmpty || $0.channels.contains(0)) }
    }

    private static let brushes: [(label: String, value: Brush)] = [
        ("Point", .point), ("Narrow", .narrow), ("Wide", .wide),
    ]

    private var graph: EqGraph {
        let onEdit: ((EqGraph.Edit) -> Void)? = editable ? { edit($0) } : nil
        return EqGraph(
            response: live ?? response,
            parts: parts,
            handles: editable ? BandTable.handles(detail.bands) : [],
            paints: editable && curveIndex != nil,
            onEdit: onEdit
        )
    }

    var body: some View {
        VStack(alignment: .leading, spacing: KoanTheme.Space.m) {
            graph
            #if !os(tvOS)
            if editable, curveIndex != nil {
                KoanSegmentedPicker(options: Self.brushes, selection: $brush, title: "Brush")
            }
            #endif
        }
        .onChange(of: response.total) { _, _ in
            if !editing {
                live = nil
                painted = nil
            }
        }
        .onDisappear {
            unsaved?()
            unsaved = nil
            editing = false
            waiting = nil
        }
    }

    private func edit(_ e: EqGraph.Edit) {
        switch e {
        case let .band(h, done):
            guard detail.bands.indices.contains(h.index) else { return }
            let kind = detail.bands[h.index].kind
            step(done) {
                await dsp.previewBand(name, h.index, kind: kind, freq: h.hz, gain: h.db, q: h.q)
            } save: {
                dsp.setBand(name, h.index, kind: kind, freq: h.hz, gain: h.db, q: h.q)
            }
        case let .paint(hz, db, done):
            guard let i = curveIndex else { return }
            let points = paint(painted ?? detail.bands[i].curve, at: hz, to: db)
            painted = points
            step(done) {
                await dsp.previewCurve(name, i, points)
            } save: {
                dsp.setCurve(name, i, points)
            }
        }
    }

    /// Draw a step of an edit at once, and save it if a quarter second has
    /// passed since the last save, or it is the last step. The first step
    /// waits too: a touch that was the start of a scroll saves nothing.
    private func step(_ done: Bool, preview: @escaping () async -> DspResponse?, save: @escaping () -> Void) {
        if !editing { saved = .now }
        editing = !done
        if done || Date.now.timeIntervalSince(saved) > 0.25 {
            save()
            saved = .now
            unsaved = nil
        } else {
            unsaved = save
        }
        if done {
            waiting = nil
        } else if drawing {
            waiting = preview
        } else {
            draw(preview)
        }
    }

    private func draw(_ preview: @escaping () async -> DspResponse?) {
        drawing = true
        Task {
            let drawn = await preview()
            if editing, let drawn { live = drawn }
            if let next = waiting {
                waiting = nil
                draw(next)
            } else {
                drawing = false
            }
        }
    }

    /// The curve pulled toward `db` at `hz`: the nearest point alone, or its
    /// neighbours too, less the further they are, over a third of an octave
    /// or an octave.
    private func paint(_ points: [DspPoint], at hz: Double, to db: Double) -> [DspPoint] {
        let octaves = { (p: DspPoint) in abs(log2(p.hz / hz)) }
        switch brush {
        case .point:
            guard let i = points.indices.min(by: { octaves(points[$0]) < octaves(points[$1]) }) else { return points }
            var out = points
            out[i] = DspPoint(hz: points[i].hz, db: db)
            return out
        case .narrow, .wide:
            let width = brush == .narrow ? 1.0 / 3 : 1.0
            return points.map { p in
                let d = octaves(p) / width
                return DspPoint(hz: p.hz, db: p.db + exp(-d * d) * (db - p.db))
            }
        }
    }
}
