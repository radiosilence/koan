import Foundation

@Observable
final class ServerConfig {
    static let shared = ServerConfig()

    private static let urlKey = "koan_server_url"
    private static let defaultURL = "http://localhost:3694"

    var serverURL: String {
        didSet { UserDefaults.standard.set(serverURL, forKey: Self.urlKey) }
    }

    var baseURL: URL? { URL(string: serverURL) }

    var graphQLURL: URL? { baseURL?.appendingPathComponent("graphql") }

    private init() {
        self.serverURL = UserDefaults.standard.string(forKey: Self.urlKey) ?? Self.defaultURL
    }
}
