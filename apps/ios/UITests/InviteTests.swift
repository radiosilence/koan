import XCTest

/// Opens an invite and waits for the library: the one tap an invited listener
/// makes, and nothing after it.
///
/// `just ios-join` passes the link in; without one the test is skipped, so
/// `ios-walk` can run the target as a whole.
@MainActor
final class InviteTests: XCTestCase {
    func testInviteJoins() throws {
        guard let link = ProcessInfo.processInfo.environment["KOAN_INVITE_LINK"],
              let url = URL(string: link)
        else { throw XCTSkip("no invite given") }

        let app = XCUIApplication()
        // Silent, and blind to UPnP renderers: tests share a machine with its owner.
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        app.launch()
        app.open(url)
        // A custom scheme opened from outside the app asks first.
        let open = XCUIApplication(bundleIdentifier: "com.apple.springboard").buttons[any: "Open"]
        if open.waitForExistence(timeout: 5) { open.tap() }

        XCTAssert(app.images.firstMatch.waitForExistence(timeout: 180), "no albums after joining")
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.lifetime = .keepAlways
        add(shot)
    }
}
