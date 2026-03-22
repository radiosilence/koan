import Foundation

// MARK: - GraphQL query/mutation strings

enum GQL {
    static let nowPlaying = """
    query NowPlaying {
        nowPlaying {
            state
            positionMs
            durationMs
            queueItemId
            track {
                title
                artist
                album
                codec
                sampleRate
                bitDepth
                channels
                durationMs
            }
        }
    }
    """

    static let queue = """
    query Queue {
        queue {
            queueItemId
            title
            artist
            album
            codec
            trackNumber
            disc
            durationMs
            isCurrent
        }
    }
    """

    static let play = """
    mutation Play($queueItemId: String!) {
        play(queueItemId: $queueItemId) { ok message }
    }
    """

    static let pause = """
    mutation Pause {
        pause { ok message }
    }
    """

    static let resume = """
    mutation Resume {
        resume { ok message }
    }
    """

    static let stop = """
    mutation Stop {
        stop { ok message }
    }
    """

    static let next = """
    mutation Next {
        next { ok message }
    }
    """

    static let previous = """
    mutation Previous {
        previous { ok message }
    }
    """

    static let seek = """
    mutation Seek($positionMs: Int!) {
        seek(positionMs: $positionMs) { ok message }
    }
    """

    static let removeFromQueue = """
    mutation RemoveFromQueue($queueItemIds: [String!]!) {
        removeFromQueue(queueItemIds: $queueItemIds) { ok message }
    }
    """

    static let moveInQueue = """
    mutation MoveInQueue($queueItemIds: [String!]!, $target: String!, $after: Boolean!) {
        moveInQueue(queueItemIds: $queueItemIds, target: $target, after: $after) { ok message }
    }
    """

    static let clearQueue = """
    mutation ClearQueue {
        clearQueue { ok message }
    }
    """

    static let toggleFavourite = """
    mutation ToggleFavourite($trackId: Int!) {
        toggleFavourite(trackId: $trackId) { id isFavourite }
    }
    """
}
