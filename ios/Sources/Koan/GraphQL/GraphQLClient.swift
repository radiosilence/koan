import Foundation

struct GraphQLClient: Sendable {
    private let session = URLSession.shared
    let baseURL: URL

    func execute<T: Decodable>(query: String, variables: [String: Any] = [:]) async throws -> T {
        let url = baseURL.appendingPathComponent("graphql")
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")

        let body: [String: Any] = [
            "query": query,
            "variables": variables
        ]
        request.httpBody = try JSONSerialization.data(withJSONObject: body)

        let (data, response) = try await session.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse else {
            throw GraphQLError.invalidResponse
        }
        guard (200...299).contains(httpResponse.statusCode) else {
            throw GraphQLError.httpError(httpResponse.statusCode)
        }

        let decoded = try JSONDecoder().decode(GraphQLResponse<T>.self, from: data)
        if let errors = decoded.errors, !errors.isEmpty {
            throw GraphQLError.graphQL(errors.map(\.message))
        }
        guard let result = decoded.data else {
            throw GraphQLError.noData
        }
        return result
    }

    // MARK: - Convenience mutations

    func addToQueue(trackIds: [Int64]) async {
        let variables: [String: Any] = ["trackIds": trackIds]
        let _: QueueMutationResponse? = try? await execute(
            query: GQL.addToQueue, variables: variables
        )
    }

    func replaceQueue(trackIds: [Int64]) async {
        guard !trackIds.isEmpty else { return }
        let variables: [String: Any] = ["trackIds": trackIds]
        let _: QueueMutationResponse? = try? await execute(
            query: GQL.replaceQueue, variables: variables
        )
    }

    func toggleFavourite(trackId: Int64) async {
        let variables: [String: Any] = ["trackId": trackId]
        let _: ToggleFavouriteResponse? = try? await execute(
            query: GQL.toggleFavourite, variables: variables
        )
    }
}

struct GraphQLResponse<T: Decodable>: Decodable {
    let data: T?
    let errors: [GraphQLResponseError]?
}

struct GraphQLResponseError: Decodable {
    let message: String
}

enum GraphQLError: LocalizedError {
    case invalidResponse
    case httpError(Int)
    case graphQL([String])
    case noData

    var errorDescription: String? {
        switch self {
        case .invalidResponse: "Invalid server response"
        case .httpError(let code): "HTTP \(code)"
        case .graphQL(let messages): messages.joined(separator: "\n")
        case .noData: "No data returned"
        }
    }
}
