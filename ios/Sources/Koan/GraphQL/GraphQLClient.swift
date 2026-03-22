import Foundation

enum GraphQLError: LocalizedError {
    case noServerURL
    case httpError(Int)
    case networkError(Error)
    case graphQLErrors([GraphQLResponseError])
    case decodingError(Error)

    var errorDescription: String? {
        switch self {
        case .noServerURL:
            "Server URL not configured"
        case .httpError(let code):
            "HTTP \(code)"
        case .networkError(let error):
            error.localizedDescription
        case .graphQLErrors(let errors):
            errors.map(\.message).joined(separator: ", ")
        case .decodingError(let error):
            "Decode failed: \(error.localizedDescription)"
        }
    }
}

struct GraphQLResponseError: Decodable {
    let message: String
}

struct GraphQLResponse<T: Decodable>: Decodable {
    let data: T?
    let errors: [GraphQLResponseError]?
}

final class GraphQLClient: Sendable {
    static let shared = GraphQLClient()

    private let session: URLSession
    private let decoder: JSONDecoder

    init(session: URLSession = .shared) {
        self.session = session
        self.decoder = JSONDecoder()
    }

    func execute<T: Decodable>(
        query: String,
        variables: [String: Any]? = nil
    ) async throws -> T {
        guard let url = ServerConfig.shared.graphQLURL else {
            throw GraphQLError.noServerURL
        }

        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")

        var body: [String: Any] = ["query": query]
        if let variables { body["variables"] = variables }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)

        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            throw GraphQLError.networkError(error)
        }

        if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode) {
            throw GraphQLError.httpError(http.statusCode)
        }

        let gqlResponse: GraphQLResponse<T>
        do {
            gqlResponse = try decoder.decode(GraphQLResponse<T>.self, from: data)
        } catch {
            throw GraphQLError.decodingError(error)
        }

        if let errors = gqlResponse.errors, !errors.isEmpty {
            throw GraphQLError.graphQLErrors(errors)
        }

        guard let result = gqlResponse.data else {
            throw GraphQLError.graphQLErrors([GraphQLResponseError(message: "No data in response")])
        }

        return result
    }
}
