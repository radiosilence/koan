import Foundation
import SwiftUI

@Observable
final class ServerConfig {
    var host: String {
        didSet {
            UserDefaults.standard.set(host, forKey: "koan_server_host")
            rebuildClient()
        }
    }
    var port: Int {
        didSet {
            UserDefaults.standard.set(port, forKey: "koan_server_port")
            rebuildClient()
        }
    }

    var baseURL: URL {
        URL(string: "http://\(host):\(port)")!
    }

    private(set) var client: GraphQLClient

    init() {
        let host = UserDefaults.standard.string(forKey: "koan_server_host") ?? "localhost"
        let port = UserDefaults.standard.integer(forKey: "koan_server_port").nonZero ?? 4000
        self.host = host
        self.port = port
        self.client = GraphQLClient(baseURL: URL(string: "http://\(host):\(port)")!)
    }

    private func rebuildClient() {
        client = GraphQLClient(baseURL: baseURL)
    }
}

private extension Int {
    var nonZero: Int? { self == 0 ? nil : self }
}
