import XCTest

/// Opens an invite on the television and waits for it to sign in: what pasting
/// the link into Settings does, without the keyboard.
///
/// The invite arrives as koan.rocks' link; a television has no browser to open
/// that in, so it is handed to the app as `koan://join`, which carries the same
/// account in its fragment. `just tv-join` passes the link in; without one the
/// test is skipped.
@MainActor
final class TVInviteTests: XCTestCase {
    func testInviteJoins() throws {
        guard let link = ProcessInfo.processInfo.environment["KOAN_INVITE_LINK"],
              let fragment = URL(string: link)?.fragment,
              let url = URL(string: "koan://join#\(fragment)")
        else { throw XCTSkip("no invite given") }

        let app = XCUIApplication()
        // Silent, and blind to UPnP renderers: tests share a machine with its owner.
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        // Nothing here needs the local network; see `TVWalkTests`.
        app.launchEnvironment["KOAN_DEVICES__NEARBY"] = "false"
        app.launch()
        app.open(url)

        // Joined, the library syncs; its first records are what shows it worked.
        XCTAssertTrue(app.buttons["Library"].waitForExistence(timeout: 30))
        sleep(20)
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.lifetime = .keepAlways
        add(shot)
    }
}
