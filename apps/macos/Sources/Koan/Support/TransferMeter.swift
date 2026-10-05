#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
import Foundation
import KoanFFI
import Observation

/// Something on screen drawing one transfer's progress.
@MainActor
protocol TransferGauge: AnyObject {
    func take(_ figure: TransferFigure)
}

/// Something on screen drawing several transfers at once: a record's bar, as
/// its tracks download.
@MainActor
protocol RecordGauge: AnyObject {
    func take(_ figures: [TransferFigure])
}

/// Download progress at the display's rate, for whatever is drawing it.
///
/// The mirror's `Figures` slice moves when the engine samples a transfer's
/// rate, a few times a second, so a ring fed from it moves in steps. This reads
/// the byte counts on every frame instead and hands each one straight to the
/// gauges drawing it, as `PlayingLevels` does with the analyser: no SwiftUI
/// body runs for it and no table reconfigures a row.
///
/// The display link exists only while a gauge is attached and a transfer is
/// running. A koan with nothing downloading, or nothing on screen showing a
/// download, has no link and reads nothing.
///
/// `Observable` by conformance alone, so it can be handed down the
/// environment.
@MainActor
final class TransferMeter: Observable {
    private let engine: KoanEngine

    /// Each gauge and the transfers it draws, by track. Weak keys, so a row
    /// that scrolls away is forgotten without having to say goodbye.
    private let gauges = NSMapTable<AnyObject, NSArray>.weakToStrongObjects()

    /// The last figure handed out per transfer, so a frame in which nothing
    /// arrived touches no layer.
    private var latest: [Int64: TransferFigure] = [:]

    private var running = false
    private var away = false
    private var link: CADisplayLink?

    init(engine: KoanEngine, mirror: EngineMirror) {
        self.engine = engine
        mirror.follow { [weak self, weak mirror] in
            guard let self, let mirror else { return }
            running = mirror.transfers.contains { $0.state == .running }
            if !running { latest.removeAll() }
            relink()
        }
        #if !canImport(AppKit)
        for (name, away) in [
            (UIApplication.didEnterBackgroundNotification, true),
            (UIApplication.willEnterForegroundNotification, false),
        ] {
            NotificationCenter.default.addObserver(
                forName: name, object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.away = away
                    self?.relink()
                }
            }
        }
        #endif
    }

    /// Have `gauge` draw `transfer`, or nothing when it is `nil`. Called
    /// whenever a row is configured; it is handed the current figure at once,
    /// so it never draws an old one while waiting for a frame. A row that is
    /// mounted but off stage — the queue behind another page — passes `nil`,
    /// or the link would run for a ring nobody can see.
    func follow(_ gauge: TransferGauge, transfer: Int64?) {
        guard let transfer else {
            gauges.removeObject(forKey: gauge)
            relink()
            return
        }
        gauges.setObject([NSNumber(value: transfer)], forKey: gauge)
        if let figure = figure(for: transfer) { gauge.take(figure) }
        relink()
    }

    /// Have `gauge` draw `transfers`, or nothing when there are none.
    func follow(_ gauge: RecordGauge, transfers: [Int64]) {
        guard !transfers.isEmpty else {
            gauges.removeObject(forKey: gauge)
            relink()
            return
        }
        gauges.setObject(transfers.map { NSNumber(value: $0) } as NSArray, forKey: gauge)
        if link == nil { read() }
        gauge.take(transfers.compactMap { latest[$0] })
        relink()
    }

    /// How far a transfer has got, for a row deciding how to draw it. The last
    /// frame's figure while the link runs, otherwise read now.
    func figure(for transfer: Int64) -> TransferFigure? {
        if link == nil { read() }
        return latest[transfer]
    }

    private func read() {
        latest = Dictionary(
            engine.transferReadings().map { ($0.trackId, $0) },
            uniquingKeysWith: { first, _ in first }
        )
    }

    /// The gauges still alive. A weak-keyed map table drops a dead key
    /// lazily, so its `count` can go on counting rows that are gone.
    private var live: [AnyObject] {
        gauges.keyEnumerator().allObjects.map { $0 as AnyObject }
    }

    private func relink() {
        let wanted = running && !away && !live.isEmpty
        if wanted, link == nil {
            link = makeLink()
        } else if !wanted, let link {
            link.invalidate()
            self.link = nil
        }
    }

    private func makeLink() -> CADisplayLink? {
        #if canImport(AppKit)
        let main = NSApp.windows.first { $0.identifier?.rawValue == MainWindow.id }
        guard let window = main ?? NSApp.keyWindow else { return nil }
        let link = window.displayLink(target: Tick(self), selector: #selector(Tick.fire))
        #else
        let link = CADisplayLink(target: Tick(self), selector: #selector(Tick.fire))
        #endif
        // A ring twelve points across gains nothing above 60, and a phone pays
        // for every frame.
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 60, preferred: 60)
        link.add(to: .main, forMode: .common)
        return link
    }

    /// A frame. A gauge is torn down without saying so — a table goes with
    /// the page that held it — so the link checks for itself that someone is
    /// still listening, and stops when nobody is.
    fileprivate func tick() {
        let gauges = live
        guard !gauges.isEmpty else { return relink() }
        let before = latest
        read()
        for gauge in gauges {
            guard let ids = (self.gauges.object(forKey: gauge) as? [NSNumber])?.map(\.int64Value),
                  ids.contains(where: { latest[$0] != before[$0] })
            else { continue }
            if let gauge = gauge as? TransferGauge, let figure = latest[ids[0]] {
                gauge.take(figure)
            } else if let gauge = gauge as? RecordGauge {
                gauge.take(ids.compactMap { latest[$0] })
            }
        }
    }
}

/// The display link's target. A link holds its target strongly until it is
/// invalidated; this keeps that from being the meter.
private final class Tick: NSObject {
    weak var meter: TransferMeter?

    init(_ meter: TransferMeter) { self.meter = meter }

    @MainActor @objc func fire() { meter?.tick() }
}
