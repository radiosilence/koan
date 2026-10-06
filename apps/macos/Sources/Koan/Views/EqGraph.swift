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
/// be dragged; `onDrag` is told where it was let go, and the curves follow
/// once the profile has been changed and drawn again.
struct EqGraph: View {
    let response: DspResponse
    var handles: [Handle] = []
    var onDrag: ((Int, Double, Double) -> Void)?
    /// The view to open on, where there is a measurement to show.
    var startOn: Shown = .eq

    /// A band's point: its index among the profile's filters, and where it is.
    struct Handle: Identifiable, Equatable {
        let index: Int
        let hz: Double
        let db: Double
        var id: Int { index }
    }

    @State private var view: Shown = .eq
    /// The handle being dragged, and where it is now.
    @State private var dragging: Handle?
    /// Whether a drag is under way, as against one let go and not yet drawn.
    @State private var grabbed = false

    enum Shown: String, CaseIterable, Identifiable {
        case eq = "EQ"
        case headphone = "Headphone"
        var id: Self { self }
    }

    private var measured: Bool { response.measurement != nil }
    private var showingEq: Bool { view == .eq || !measured }
    private static let accent = Color.koanAccent

    var body: some View {
        content.onAppear { view = startOn }
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 10) {
            if measured {
                Picker("Show", selection: $view) {
                    ForEach(Shown.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
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
                    .foregroundStyle(Color.secondary.opacity(0.4))
                    .lineStyle(StrokeStyle(lineWidth: 0.5))
                ForEach(bandAreas) { area in
                    AreaMark(
                        x: .value("Hz", area.hz),
                        yStart: .value("dB", 0.0),
                        yEnd: .value("dB", area.db),
                        series: .value("Band", area.series)
                    )
                    .foregroundStyle(Self.accent.opacity(0.13))
                }
                // A chain with a correction and tuning: each in its role's
                // colour, under the two together.
                if let correction = response.correction, let tuning = response.tuning {
                    lines(curves: [Curve(name: "Correction", db: correction)],
                          color: ProfileRole.correction.color.opacity(0.7), width: 1.2, dashed: true)
                    lines(curves: [Curve(name: "Tuning", db: tuning)],
                          color: ProfileRole.tuning.color, width: 1.2)
                }
                lines(curves: [Curve(name: "EQ", db: response.total)], color: Self.accent, width: 2)
                ForEach(shownHandles) { h in
                    PointMark(x: .value("Hz", h.hz), y: .value("dB", h.db))
                        .symbolSize(grabbed && h.index == dragging?.index ? 120 : 60)
                        .foregroundStyle(Self.accent)
                }
            } else {
                lines(curves: response.measurement.map { [Curve(name: "Measured", db: $0)] } ?? [],
                      color: Color.secondary, width: 1.2)
                lines(curves: response.target.map { [Curve(name: "Target", db: $0)] } ?? [],
                      color: Color.primary.opacity(0.55), width: 1.2, dashed: true)
                lines(curves: response.predicted.map { [Curve(name: "Corrected", db: $0)] } ?? [],
                      color: Self.accent, width: 2)
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
        .onChange(of: handles) { _, _ in dragging = nil }
        #if !os(tvOS)
        .chartOverlay { proxy in
            if showingEq, onDrag != nil {
                GeometryReader { geo in
                    Rectangle()
                        .fill(Color.clear)
                        .contentShape(Rectangle())
                        .gesture(drag(proxy, geo))
                }
            }
        }
        #endif
        .accessibilityLabel(showingEq ? "EQ response" : "Headphone response")
    }

    private struct Curve {
        let name: String
        let db: [Double]
    }

    private func lines(
        curves: [Curve],
        color: Color,
        width: CGFloat,
        dashed: Bool = false
    ) -> some ChartContent {
        ForEach(curves.flatMap { curve in points(curve.db).map { (curve.name, $0) } }, id: \.1.id) { name, p in
            LineMark(x: .value("Hz", p.hz), y: .value("dB", p.db), series: .value("Curve", name))
                .foregroundStyle(color)
                .lineStyle(StrokeStyle(lineWidth: width, dash: dashed ? [4, 3] : []))
        }
    }

    // MARK: - Dragging a band

    private var shownHandles: [Handle] {
        handles.map { h in dragging?.index == h.index ? dragging! : h }
    }

    #if !os(tvOS)
    private func drag(_ proxy: ChartProxy, _ geo: GeometryProxy) -> some Gesture {
        DragGesture(minimumDistance: 2)
            .onChanged { g in
                guard let plot = proxy.plotFrame else { return }
                let origin = geo[plot].origin
                let at = CGPoint(x: g.location.x - origin.x, y: g.location.y - origin.y)
                if !grabbed {
                    grabbed = true
                    let start = CGPoint(x: g.startLocation.x - origin.x, y: g.startLocation.y - origin.y)
                    dragging = nearest(to: start, proxy)
                }
                guard let held = dragging,
                      let hz: Double = proxy.value(atX: at.x),
                      let db: Double = proxy.value(atY: at.y)
                else { return }
                dragging = Handle(
                    index: held.index,
                    hz: min(max(hz, 20), 20000),
                    db: min(max(db, yDomain.lowerBound), yDomain.upperBound)
                )
            }
            .onEnded { _ in
                // Held where it was let go until the edited profile is drawn.
                grabbed = false
                if let held = dragging { onDrag?(held.index, held.hz, held.db) }
            }
    }
    #endif

    /// The handle under `point`, within a finger's width of it.
    private func nearest(to point: CGPoint, _ proxy: ChartProxy) -> Handle? {
        handles
            .compactMap { h -> (Handle, CGFloat)? in
                guard let at = proxy.position(for: (x: h.hz, y: h.db)) else { return nil }
                return (h, hypot(at.x - point.x, at.y - point.y))
            }
            .filter { $0.1 < 24 }
            .min { $0.1 < $1.1 }?
            .0
    }

    // MARK: - The legend

    @ViewBuilder private var legend: some View {
        HStack(spacing: 14) {
            if showingEq {
                key("EQ", Self.accent)
                if response.correction != nil, response.tuning != nil {
                    key("Correction", ProfileRole.correction.color.opacity(0.7), dashed: true)
                    key("Tuning", ProfileRole.tuning.color)
                }
                if !response.bands.isEmpty { key("Each band", Self.accent.opacity(0.3)) }
            } else {
                key("Measured", Color.secondary)
                key("Target", Color.primary.opacity(0.55), dashed: true)
                key("Corrected", Self.accent)
            }
            Spacer()
            Text("Preamp \(String(format: "%.1f", response.preampDb)) dB")
                .monospacedDigit()
                .foregroundStyle(.secondary)
        }
        .font(.caption)
    }

    private func key(_ name: String, _ color: Color, dashed: Bool = false) -> some View {
        HStack(spacing: 5) {
            Capsule()
                .stroke(color, style: StrokeStyle(lineWidth: 2, dash: dashed ? [3, 2] : []))
                .frame(width: 14, height: 2)
            Text(name).foregroundStyle(.secondary)
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
            return [response.total, response.correction ?? [], response.tuning ?? []]
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
