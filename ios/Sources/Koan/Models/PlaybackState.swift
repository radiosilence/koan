import Foundation

enum PlaybackState: String, Codable {
    case stopped = "STOPPED"
    case playing = "PLAYING"
    case paused = "PAUSED"
}
