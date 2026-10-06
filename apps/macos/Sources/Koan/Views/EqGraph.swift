import Charts
import KoanFFI
import SwiftUI

/// What a profile does to the sound, drawn on a log-frequency axis: each of
/// its bands as a faint shape, and what they sum to over them in the accent.
/// For a correction from AutoEQ, a second view draws the headphone as
/// measured, the target it plays to, and the measurement corrected. Every
/// curve comes from the core, computed from the filters the DSP runs; this
/// only draws them.
struct EqGraph: View {
    let response: DspResponse
    @State private var view: Shown = .eq

    enum Shown: String, CaseIterable, Identifiable {
        case eq = "EQ"
        case headphone = "Headphone"
        var id: Self { self }
    }

    private var measured: Bool { response.measurement != nil }

    var body: some View {
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
            if view == .eq || !measured {
                RuleMark(y: .value("0 dB", 0))
                    .foregroundStyle(.secondary.opacity(0.4))
                    .lineStyle(StrokeStyle(lineWidth: 0.5))
                ForEach(Array(response.bands.enumerated()), id: \.offset) { index, band in
                    ForEach(points(band.db), id: \.hz) { p in
                        AreaMark(
                            x: .value("Hz", p.hz),
                            yStart: .value("dB", 0.0),
                            yEnd: .value("dB", p.db),
                            series: .value("Band", "band-\(index)")
                        )
                        .foregroundStyle(Color.koanAccent.opacity(0.13))
                        .interpolationMethod(.monotone)
                    }
                }
                line(response.total, "EQ", .koanAccent, width: 2)
            } else {
                if let m = response.measurement { line(m, "Measured", .secondary, width: 1.2) }
                if let t = response.target { line(t, "Target", .primary.opacity(0.55), width: 1.2, dashed: true) }
                if let p = response.predicted { line(p, "Corrected", .koanAccent, width: 2) }
            }
        }
        .chartXScale(domain: 20.0 ... 20000.0, type: .log)
        .chartYScale(domain: yDomain)
        .chartXAxis {
            AxisMarks(values: [20.0, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000]) { value in
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
        .accessibilityLabel(view == .eq ? "EQ response" : "Headphone response")
    }

    @ChartContentBuilder
    private func line(
        _ db: [Double],
        _ name: String,
        _ color: some ShapeStyle,
        width: CGFloat,
        dashed: Bool = false
    ) -> some ChartContent {
        ForEach(points(db), id: \.hz) { p in
            LineMark(x: .value("Hz", p.hz), y: .value("dB", p.db), series: .value("Curve", name))
                .foregroundStyle(color)
                .lineStyle(StrokeStyle(lineWidth: width, dash: dashed ? [4, 3] : []))
                .interpolationMethod(.monotone)
        }
    }

    // MARK: - The legend

    @ViewBuilder private var legend: some View {
        HStack(spacing: 14) {
            if view == .eq || !measured {
                key("EQ", .koanAccent)
                if !response.bands.isEmpty { key("Each band", Color.koanAccent.opacity(0.3)) }
            } else {
                key("Measured", .secondary)
                key("Target", .primary.opacity(0.55), dashed: true)
                key("Corrected", .koanAccent)
            }
            Spacer()
            Text("Preamp \(String(format: "%.1f", response.preampDb)) dB")
                .monospacedDigit()
                .foregroundStyle(.secondary)
        }
        .font(.caption)
    }

    private func key(_ name: String, _ color: some ShapeStyle, dashed: Bool = false) -> some View {
        HStack(spacing: 5) {
            Capsule()
                .stroke(color, style: StrokeStyle(lineWidth: 2, dash: dashed ? [3, 2] : []))
                .frame(width: 14, height: 2)
            Text(name).foregroundStyle(.secondary)
        }
    }

    // MARK: - Data

    struct Point { let hz: Double; let db: Double }

    /// Every third point of AutoEQ's grid: about 230, a pixel apart at
    /// most widths, and a third of the marks to lay out.
    private func points(_ db: [Double]) -> [Point] {
        stride(from: 0, to: min(db.count, response.freqs.count), by: 3).map {
            Point(hz: response.freqs[$0], db: db[$0])
        }
    }

    private var shown: [[Double]] {
        if view == .eq || !measured { return [response.total] + response.bands.map(\.db) }
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

    static func hzLabel(_ hz: Double) -> String {
        hz >= 1000 ? "\(Int(hz / 1000))k" : "\(Int(hz))"
    }
}
