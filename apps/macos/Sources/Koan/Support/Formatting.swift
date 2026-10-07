import Foundation
import KoanFFI

enum Format {
    /// A title without the leading date DJ-mix uploads carry ("2022-03-12:
    /// BBC Radio 1 Essential Mix"), whose year is shown beside it anyway.
    /// Display only: the tags keep it.
    static func title(_ raw: String) -> String {
        guard let r = raw.range(of: #"^\d{4}-\d{2}-\d{2}\s*[:\-–]\s*"#, options: .regularExpression)
        else { return raw }
        let rest = raw[r.upperBound...]
        return rest.isEmpty ? raw : String(rest)
    }

    /// How many lines a title may take in a list. A phone is narrow enough
    /// that one line cuts most mix and live-set titles down to their date.
    #if os(iOS) || os(tvOS)
    static let titleLines = 2
    #else
    static let titleLines = 1
    #endif

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
    /// claim this player cannot afford to get wrong. DSP is named for the same
    /// reason: "FLAC 24/96 → 48 · EQ + FIR".
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
        if let dsp = f.dsp {
            parts.append("· \(dspLabel(dsp))")
        }
        return parts.joined(separator: " ")
    }

    private static func dspLabel(_ d: DspInfo) -> String {
        switch (d.eq, d.convolutionRate != nil) {
        case (true, true): "EQ + FIR"
        case (false, true): "FIR"
        default: "EQ"
        }
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
        guard let dsp = f.dsp else { return deviceExplanation(f) }
        var what = dsp.eq && dsp.convolutionRate != nil
            ? "equalised and convolved"
            : dsp.eq ? "equalised" : "convolved"
        if let conv = dsp.convolutionRate, conv != f.sampleRate {
            what += ", resampled \(rate(f.sampleRate)) kHz → \(rate(conv)) kHz for convolution"
        }
        return "Processed by \u{201C}\(dsp.profile)\u{201D}: \(what)"
    }

    private static func deviceExplanation(_ f: StreamFormat) -> String {
        guard let out = f.outputSampleRate else {
            return "Source format — kōan matches the device to the source rate rather than resampling"
        }
        return out == f.sampleRate
            ? "Device is running at \(rate(out)) kHz, the source rate — kōan is resampling nothing"
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
    /// "0 KB" rather than the formatter's "Zero KB", which reads as a phrase
    /// beside a button.
    static func bytes(_ count: Int64) -> String {
        byteFormatter.string(fromByteCount: count)
    }

    private nonisolated(unsafe) static let byteFormatter: ByteCountFormatter = {
        let f = ByteCountFormatter()
        f.countStyle = .file
        f.allowsNonnumericFormatting = false
        return f
    }()

    static func count(_ n: Int64, _ singular: String, _ plural: String? = nil) -> String {
        let word = n == 1 ? singular : (plural ?? singular + "s")
        return "\(n.formatted(.number)) \(word)"
    }
}
