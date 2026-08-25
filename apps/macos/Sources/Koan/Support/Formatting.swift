import Foundation
import KoanFFI

enum Format {
    /// `m:ss`, or `h:mm:ss` once it earns the hour.
    static func duration(_ ms: Int64?) -> String {
        guard let ms, ms > 0 else { return "--:--" }
        return duration(UInt64(ms))
    }

    static func duration(_ ms: UInt64) -> String {
        let total = Int(ms / 1000)
        let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60)
        return h > 0
            ? String(format: "%d:%02d:%02d", h, m, s)
            : String(format: "%d:%02d", m, s)
    }

    /// What the source is: "FLAC 24/96" and so on.
    ///
    /// A rate the device refused is appended — "FLAC 24/96 → 48" — because a
    /// badge that reads the same whether or not something resampled is the one
    /// claim this player cannot afford to get wrong.
    static func quality(_ f: StreamFormat) -> String {
        var parts = [f.codec.uppercased()]
        if let depth = f.bitDepth {
            parts.append("\(depth)/\(rate(f.sampleRate))")
        } else {
            parts.append("\(rate(f.sampleRate)) kHz")
        }
        if f.channels != 2 {
            parts.append("\(f.channels)ch")
        }
        if isResampled(f), let out = f.outputSampleRate {
            parts.append("→ \(rate(out))")
        }
        return parts.joined(separator: " ")
    }

    /// Whether anything had to resample to reach the device. `false` while the
    /// device rate is unknown — silence is better than a guess here.
    static func isResampled(_ f: StreamFormat) -> Bool {
        guard let out = f.outputSampleRate else { return false }
        return out != f.sampleRate
    }

    /// What koan can honestly say about the path to the DAC. It never claims
    /// bit-perfection outright: the device is shared, so another app's audio
    /// and the system volume stage are both past the point koan can see.
    static func outputExplanation(_ f: StreamFormat) -> String {
        guard let out = f.outputSampleRate else {
            return "Source format — koan matches the device to the source rate rather than resampling"
        }
        return out == f.sampleRate
            ? "Device is running at \(rate(out)) kHz, the source rate — koan is resampling nothing"
            : "Device stayed at \(rate(out)) kHz, so \(rate(f.sampleRate)) kHz is being resampled to reach it"
    }

    static func quality(_ t: Track) -> String? {
        guard let codec = t.codec else { return nil }
        var s = codec.uppercased()
        if let depth = t.bitDepth, let sr = t.sampleRate {
            s += " \(depth)/\(rate(UInt32(sr)))"
        } else if let sr = t.sampleRate {
            s += " \(rate(UInt32(sr))) kHz"
        }
        return s
    }

    /// 44100 → "44.1", 96000 → "96". Trailing ".0" is noise.
    private static func rate(_ hz: UInt32) -> String {
        let khz = Double(hz) / 1000
        return khz == khz.rounded()
            ? String(format: "%.0f", khz)
            : String(format: "%.1f", khz)
    }

    /// Sizes the way Finder writes them — GB not GiB, because that is what the
    /// rest of the system shows and a disagreement here just looks wrong.
    static func bytes(_ count: Int64) -> String {
        ByteCountFormatter.string(fromByteCount: count, countStyle: .file)
    }

    static func count(_ n: Int64, _ singular: String, _ plural: String? = nil) -> String {
        let word = n == 1 ? singular : (plural ?? singular + "s")
        return "\(n.formatted(.number)) \(word)"
    }
}
