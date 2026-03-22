import Foundation

/// Wrapper for async-graphql Connection pagination responses.
struct PageInfo: Decodable {
    let hasPreviousPage: Bool
    let hasNextPage: Bool
}

struct Edge<T: Decodable>: Decodable {
    let cursor: String
    let node: T
}

struct Connection<T: Decodable>: Decodable {
    let pageInfo: PageInfo
    let edges: [Edge<T>]

    var nodes: [T] { edges.map(\.node) }
    var endCursor: String? { edges.last?.cursor }
}
