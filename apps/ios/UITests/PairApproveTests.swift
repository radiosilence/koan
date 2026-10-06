import XCTest

/// Approves a pairing from the phone: opens the link a television shows as a
/// QR code, and allows the device it names.
///
/// `just tv-pair-qr` decodes the link from the television's screen and passes
/// it in, with the phone signed in through the `KOAN_REMOTE__*` environment;
/// without a link the test is skipped, so `ios-walk` can run the target as a
/// whole.
@MainActor
final class PairApproveTests: XCTestCase {
    func testApprovePairing() throws {
        let env = ProcessInfo.processInfo.environment
        guard let link = env["KOAN_PAIR_LINK"], let url = URL(string: link)
        else { throw XCTSkip("no pairing link given") }

        let app = XCUIApplication()
        for key in ["KOAN_REMOTE__ENABLED", "KOAN_REMOTE__URL", "KOAN_REMOTE__USERNAME", "KOAN_REMOTE__API_KEY"] {
            if let value = env[key] { app.launchEnvironment[key] = value }
        }
        app.launchEnvironment["KOAN_DEVICES__NEARBY"] = "false"
        app.launch()
        sleep(3)
        snap("phone-01-signed-in")

        app.open(url)
        // A custom scheme opened from outside the app asks first.
        let open = XCUIApplication(bundleIdentifier: "com.apple.springboard").buttons["Open"]
        if open.waitForExistence(timeout: 5) { open.tap() }

        let allow = app.alerts.buttons["Allow"]
        XCTAssertTrue(allow.waitForExistence(timeout: 20), "the phone asks whether to sign the device in")
        snap("phone-02-asked")
        allow.tap()
        sleep(2)
        snap("phone-03-allowed")
    }

    private func snap(_ name: String) {
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
