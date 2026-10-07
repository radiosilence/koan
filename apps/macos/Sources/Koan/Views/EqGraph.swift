import Charts
import KoanFFI
import SwiftUI

/// What a profile does to the sound, drawn on a log-frequency axis: each of
/// its bands as a faint shape, and what they sum to over them in the accent.
/// For a correction from AutoEQ, a second view draws the headphone as
/// measured, the target it plays to, and the measurement corrected. Every
/// curve comes from the core, computed from the filters the DSP runs; this
/// only draws them.
///
/// Given `handles`, each band has a point at its frequency and gain that can
/// be dragged, and its width pinched (option-dragged with a mouse) for Q.
/// With `paints`, a drag away from every handle paints a graphic curve.
/// `onEdit` is told of each step and of the end; the caller draws the edit
/// by passing another `response`.
struct EqGraph: View {
    let response: DspResponse
    /// An output's chain, stage by stage, each drawn as its block is, under
    /// the whole chain. Empty for a single profile.
    var parts: [Part] = []
    var handles: [Handle] = []
    var paints = false
    var onEdit: ((Edit) -> Void)?
    /// The view to open on, where there is a measurement to show.
    var startOn: Shown = .eq

    /// A stage of a chain: its name, its curve, and how its block draws it.
    struct Part {
        let name: String
        let db: [Double]
        let stroke: StageStroke
    }

    /// A band's point: its index among the profile's filters, where it is,
    /// and its Q.
    struct Handle: Identifiable, Equatable {
        let index: Int
        var hz: Double
        var db: Double
        var q: Double
        var id: Int { index }
    }

    /// What a gesture did: moved or widened a band, or painted the curve
    /// toward a point. `done` at the gesture's end.
    enum Edit {
        case band(Handle, done: Bool)
        case paint(hz: Double, db: Double, done: Bool)
    }

    @State private var view: Shown = .eq
    /// The handle being dragged, and where it is now.
    @State private var dragging: Handle?
    /// Whether a drag is under way, as against one let go and not yet drawn.
    @State private var grabbed = false
    /// The drag paints rather than holding a handle.
    @State private var painting = false
    /// The handle last held, which a pinch away from every handle widens.
    @State private var selected: Int?
    /// The held handle's Q when the pinch or option-drag began.
    @State private var startQ: Double?
    /// A pinch is under way: a drag's finger moves nothing meanwhile.
    @State private var pinching = false
    /// Where the brush last was, to end a stroke cut short.
    @State private var lastPaint: (hz: Double, db: Double)?
    /// A drag that began across neither a handle nor the curve, left to the
    /// page to scroll.
    @State private var passing = false
    /// Whether each gesture is still under way. A gesture cut short — the
    /// page scrolled or closed, a call — never reaches `onEnded`; this going
    /// false ends it.
    @GestureState private var dragActive = false
    @GestureState private var pinchActive = false

    enum Shown: String, CaseIterable, Identifiable {
        case eq = "EQ"
        case headphone = "Measured"
        var id: Self { self }
    }

    private var measured: Bool { response.measurement != nil }
    private var showingEq: Bool { view == .eq || !measured }

    var body: some View {
        content.onAppear { view = startOn }
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 10) {
            if measured {
                KoanSegmentedPicker(
                    options: Shown.allCases.map { ($0.rawValue, $0) },
                    selection: $view,
                    title: "Show"
                )
            }
            chart
                .frame(height: 220)
            legend
        }
    }

    // MARK: - The chart

    private var chart: some View {
        Chart {
            if showingEq {
                RuleMark(y: .value("dB", 0.0))
                    .foregroundStyle(KoanTheme.style(.rule, system: Color.secondary.opacity(0.4)))
                    .lineStyle(StrokeStyle(lineWidth: 0.5))
                ForEach(bandAreas) { area in
                    AreaMark(
                        x: .value("Hz", area.hz),
                        yStart: .value("dB", 0.0),
                        yEnd: .value("dB", area.db),
                        series: .value("Band", area.series)
                    )
                    // Each band neutral, so the accent is the curve that plays.
                    .foregroundStyle(KoanTheme.style(.muted, system: .tint).opacity(0.15))
                }
                // An output's chain: each stage as its block draws it, under
                // the whole.
                ForEach(Array(parts.enumerated()), id: \.offset) { _, part in
                    lines(curves: [Curve(name: part.name, db: part.db)],
                          color: part.stroke.style, width: 1.2, dash: part.stroke.dash)
                }
                // A chain with a correction and tuning: each in its role's
                // colour, under the two together.
                if parts.isEmpty, let correction = response.correction, let tuning = response.tuning {
                    lines(curves: [Curve(name: "Correction", db: correction)],
                          color: AnyShapeStyle(ProfileRole.correction.color.opacity(0.7)), width: 1.2, dashed: true)
                    lines(curves: [Curve(name: "Tuning", db: tuning)],
                          color: AnyShapeStyle(ProfileRole.tuning.color), width: 1.2)
                }
                // A split's preview: the baked EQ the two come from.
                if let original = response.original {
                    lines(curves: [Curve(name: "Original", db: original)],
                          color: KoanTheme.style(.muted, system: Color.secondary), width: 1.2, dashed: true)
                }
                lines(curves: [Curve(name: "EQ", db: response.total)],
                      color: parts.isEmpty ? AnyShapeStyle(.tint) : StageStroke.total.style, width: 2)
                ForEach(shownHandles) { h in
                    PointMark(x: .value("Hz", h.hz), y: .value("dB", h.db))
                        .symbolSize(grabbed && h.index == dragging?.index ? 120 : 60)
                        .foregroundStyle(.tint)
                }
            } else {
                lines(curves: response.measurement.map { [Curve(name: "Measured", db: $0)] } ?? [],
                      color: KoanTheme.style(.muted), width: 1.2)
                lines(curves: response.target.map { [Curve(name: "Target", db: $0)] } ?? [],
                      color: AnyShapeStyle(KoanTheme.style(.ink).opacity(0.55)), width: 1.2, dashed: true)
                lines(curves: response.predicted.map { [Curve(name: "Corrected", db: $0)] } ?? [],
                      color: AnyShapeStyle(.tint), width: 2)
            }
        }
        .chartXScale(domain: 20.0 ... 20000.0, type: .log)
        .chartYScale(domain: yDomain)
        .chartXAxis {
            AxisMarks(values: Self.hzTicks) { value in
                AxisGridLine()
                AxisValueLabel {
                    if let hz = value.as(Double.self) { Text(Self.hzLabel(hz)) }
                }
            }
        }
        .chartYAxis {
            AxisMarks(position: .leading, values: .stride(by: yStride)) { value in
                AxisGridLine()
                AxisValueLabel {
                    if let db = value.as(Double.self) { Text("\(Int(db))") }
                }
            }
        }
        .chartLegend(.hidden)
        // Saves land while a drag goes on; the handle stays with the finger.
        .onChange(of: handles) { _, _ in if !grabbed { dragging = nil } }
        #if !os(tvOS)
        .chartOverlay { proxy in
            if showingEq, onEdit != nil {
                GeometryReader { geo in
                    Rectangle()
                        .fill(Color.clear)
                        .contentShape(Rectangle())
                        .gesture(drag(proxy, geo).simultaneously(with: pinch(proxy, geo)))
                }
            }
        }
        .onChange(of: dragActive) { _, active in if !active { endDrag() } }
        .onChange(of: pinchActive) { _, active in if !active { endPinch() } }
        #endif
        .accessibilityLabel(showingEq ? "EQ response" : "Measured response")
    }

    private struct Curve {
        let name: String
        let db: [Double]
    }

    private func lines(
        curves: [Curve],
        color: AnyShapeStyle,
        width: CGFloat,
        dashed: Bool = false,
        dash: [CGFloat]? = nil
    ) -> some ChartContent {
        ForEach(curves.flatMap { curve in points(curve.db).map { (curve.name, $0) } }, id: \.1.id) { name, p in
            LineMark(x: .value("Hz", p.hz), y: .value("dB", p.db), series: .value("Curve", name))
                .foregroundStyle(color)
                .lineStyle(StrokeStyle(lineWidth: width, dash: dash ?? (dashed ? [4, 3] : [])))
        }
    }

    // MARK: - Dragging a band

    private var shownHandles: [Handle] {
        handles.map { h in dragging?.index == h.index ? dragging! : h }
    }

    #if !os(tvOS)
    private func drag(_ proxy: ChartProxy, _ geo: GeometryProxy) -> some Gesture {
        DragGesture(minimumDistance: 2)
            .updating($dragActive) { _, active, _ in active = true }
            .onChanged { g in
                guard let plot = proxy.plotFrame else { return }
                let origin = geo[plot].origin
                let at = CGPoint(x: g.location.x - origin.x, y: g.location.y - origin.y)
                if !grabbed, !pinching, !passing {
                    let start = CGPoint(x: g.startLocation.x - origin.x, y: g.startLocation.y - origin.y)
                    let under = nearest(to: start, proxy)
                    // A stroke starts across the curve; one starting up or
                    // down is the page being scrolled. Which it is waits for
                    // a few points of travel.
                    if under == nil, paints, hypot(g.translation.width, g.translation.height) < 8 {
                        return
                    }
                    if under == nil, !paints || abs(g.translation.height) >= abs(g.translation.width) {
                        passing = true
                        return
                    }
                    grabbed = true
                    dragging = under
                    painting = under == nil
                    selected = under?.index ?? selected
                    startQ = under?.q
                }
                guard grabbed,
                      let hz: Double = proxy.value(atX: at.x),
                      let db: Double = proxy.value(atY: at.y)
                else { return }
                let (hzIn, dbIn) = (min(max(hz, 20), 20000), min(max(db, yDomain.lowerBound), yDomain.upperBound))
                if painting {
                    lastPaint = (hzIn, dbIn)
                    onEdit?(.paint(hz: hzIn, db: dbIn, done: false))
                    return
                }
                guard !pinching, var held = dragging else { return }
                #if os(macOS)
                // Option-drag widens or narrows the band where it is: up for a
                // higher Q.
                if NSEvent.modifierFlags.contains(.option), let q = startQ {
                    held.q = Self.clampQ(q * pow(2, -g.translation.height / 60))
                    dragging = held
                    onEdit?(.band(held, done: false))
                    return
                }
                #endif
                (held.hz, held.db) = (hzIn, dbIn)
                dragging = held
                onEdit?(.band(held, done: false))
            }
            .onEnded { _ in endDrag() }
    }

    /// The drag's end, whether it was let go or cut short; once only. The
    /// handle is held where it was let go until the edited profile is drawn.
    private func endDrag() {
        passing = false
        guard grabbed else { return }
        grabbed = false
        // A pinch under way ends itself.
        if pinching { return }
        startQ = nil
        if painting {
            painting = false
            if let at = lastPaint { onEdit?(.paint(hz: at.hz, db: at.db, done: true)) }
            lastPaint = nil
        } else if let held = dragging {
            onEdit?(.band(held, done: true))
        }
    }

    /// A pinch widens the band under it, or the one last held: apart for a
    /// wider band, a lower Q.
    private func pinch(_ proxy: ChartProxy, _ geo: GeometryProxy) -> some Gesture {
        MagnifyGesture()
            .updating($pinchActive) { _, active, _ in active = true }
            .onChanged { g in
                if !pinching {
                    pinching = true
                    var under: Handle?
                    if let plot = proxy.plotFrame {
                        let origin = geo[plot].origin
                        under = nearest(to: CGPoint(x: g.startLocation.x - origin.x, y: g.startLocation.y - origin.y), proxy, within: 44)
                    }
                    guard let h = under ?? handles.first(where: { $0.index == selected }) else { return }
                    dragging = h
                    selected = h.index
                    startQ = h.q
                }
                guard var held = dragging, let q = startQ else { return }
                held.q = Self.clampQ(q / g.magnification)
                dragging = held
                onEdit?(.band(held, done: false))
            }
            .onEnded { _ in endPinch() }
    }

    /// The pinch's end, whether it was let go or cut short; once only.
    private func endPinch() {
        guard pinching else { return }
        pinching = false
        guard startQ != nil else { return }
        startQ = nil
        if let held = dragging { onEdit?(.band(held, done: true)) }
    }
    #endif

    private static func clampQ(_ q: Double) -> Double { min(max(q, 0.1), 20) }

    /// The handle under `point`, within a finger's width of it.
    private func nearest(to point: CGPoint, _ proxy: ChartProxy, within: CGFloat = 24) -> Handle? {
        handles
            .compactMap { h -> (Handle, CGFloat)? in
                guard let at = proxy.position(for: (x: h.hz, y: h.db)) else { return nil }
                return (h, hypot(at.x - point.x, at.y - point.y))
            }
            .filter { $0.1 < within }
            .min { $0.1 < $1.1 }?
            .0
    }

    // MARK: - The legend

    @ViewBuilder private var legend: some View {
        FlowLayout(spacing: 10) {
            if showingEq {
                if !parts.isEmpty {
                    ForEach(Array(parts.enumerated()), id: \.offset) { _, part in
                        key(part.name, part.stroke.style, dash: part.stroke.dash)
                    }
                    key(KoanTheme.label("Total"), StageStroke.total.style)
                } else if response.correction != nil, response.tuning != nil {
                    key(KoanTheme.label("Correction"), AnyShapeStyle(ProfileRole.correction.color.opacity(0.7)), dashed: true)
                    key(KoanTheme.label("Tuning"), AnyShapeStyle(ProfileRole.tuning.color))
                    key(KoanTheme.label("Total"), AnyShapeStyle(.tint))
                } else {
                    key(KoanTheme.label("EQ"), AnyShapeStyle(.tint))
                }
                if response.original != nil {
                    key(KoanTheme.label("Original"), KoanTheme.style(.muted, system: Color.secondary), dashed: true)
                }
                key(KoanTheme.label("No change"), KoanTheme.style(.rule, system: Color.secondary.opacity(0.4)), thin: true)
                if !response.bands.isEmpty {
                    key(KoanTheme.label("Each band"), AnyShapeStyle(KoanTheme.style(.muted, system: .tint).opacity(0.3)))
                }
            } else {
                key(KoanTheme.label("Measured"), KoanTheme.style(.muted))
                key(KoanTheme.label("Target"), AnyShapeStyle(KoanTheme.style(.ink).opacity(0.55)), dashed: true)
                key(KoanTheme.label("Corrected"), AnyShapeStyle(.tint))
            }
            Text("\(KoanTheme.label("Preamp")) \(String(format: "%.1f", response.preampDb)) dB")
                .monospacedDigit()
        }
        .koanText(.fine, .muted)
    }

    private func key(
        _ name: String, _ color: AnyShapeStyle, dashed: Bool = false, thin: Bool = false, dash: [CGFloat]? = nil
    ) -> some View {
        HStack(spacing: 5) {
            Capsule()
                .stroke(color, style: StrokeStyle(lineWidth: thin ? 1 : 2, dash: dash ?? (dashed ? [3, 2] : [])))
                .frame(width: 14, height: 2)
            Text(name)
        }
    }

    // MARK: - Data

    struct Point: Identifiable {
        let id: Int
        let hz: Double
        let db: Double
    }

    private struct Area: Identifiable {
        let id: Int
        let series: String
        let hz: Double
        let db: Double
    }

    /// Every third point of AutoEQ's grid: about 230, a pixel apart at
    /// most widths, and a third of the marks to lay out.
    private func points(_ db: [Double]) -> [Point] {
        stride(from: 0, to: min(db.count, response.freqs.count), by: 3).map {
            Point(id: $0, hz: response.freqs[$0], db: db[$0])
        }
    }

    private var bandAreas: [Area] {
        response.bands.enumerated().flatMap { band, curve in
            points(curve.db).map { p in
                Area(id: band * 10_000 + p.id, series: "band-\(band)", hz: p.hz, db: p.db)
            }
        }
    }

    private var shown: [[Double]] {
        if showingEq {
            return [response.total, response.correction ?? [], response.tuning ?? [], response.original ?? []]
                + parts.map(\.db)
                + response.bands.map(\.db) + [handles.map(\.db)]
        }
        return [response.measurement, response.target, response.predicted].compactMap { $0 }
    }

    /// Round to the next 6 dB past whatever is drawn, at least ±6 dB, so a
    /// small correction is not drawn as a large one.
    private var yDomain: ClosedRange<Double> {
        let values = shown.flatMap { $0 }
        let lo = min(-6, ((values.min() ?? 0) / 6).rounded(.down) * 6)
        let hi = max(6, ((values.max() ?? 0) / 6).rounded(.up) * 6)
        return lo ... hi
    }

    private var yStride: Double { (yDomain.upperBound - yDomain.lowerBound) > 30 ? 10 : 6 }

    private static let hzTicks: [Double] = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000]

    static func hzLabel(_ hz: Double) -> String {
        hz >= 1000 ? "\(Int(hz / 1000))k" : "\(Int(hz))"
    }
}
