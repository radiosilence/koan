import Foundation

/// "kōan 0.40.0 (1790651527)": which build this is, for someone reporting a
/// bug or checking a TestFlight update arrived.
enum AppVersion {
    static let text: String = {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? "?"
        let build = info["CFBundleVersion"] as? String
        return build.map { "kōan \(version) (\($0))" } ?? "kōan \(version)"
    }()
}
