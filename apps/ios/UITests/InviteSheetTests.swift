import XCTest

/// Makes an invite from Settings → People as an admin, and checks the sheet
/// stays up once the account list reloads behind it.
///
/// `just ios-invite-sheet` starts a throwaway server and signs the app in
/// through the `KOAN_REMOTE__*` environment; without it the test is skipped,
/// so `ios-walk` can run the target as a whole.
@MainActor
final class InviteSheetTests: XCTestCase {
    func testInviteSheetStays() throws {
        let env = ProcessInfo.processInfo.environment
        guard env["KOAN_INVITE_SHEET"] != nil else { throw XCTSkip("no server given") }

        let app = XCUIApplication()
        // Silent, and blind to UPnP renderers: tests share a machine with its owner.
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        for key in ["KOAN_REMOTE__ENABLED", "KOAN_REMOTE__URL", "KOAN_REMOTE__USERNAME", "KOAN_REMOTE__API_KEY"] {
            if let value = env[key] { app.launchEnvironment[key] = value }
        }
        app.launchEnvironment["KOAN_DEVICES__NEARBY"] = "false"
        app.launch()

        let settings = app.buttons[any: "Settings"]
        XCTAssert(settings.waitForExistence(timeout: 10), "no Settings tab")
        settings.tap()
        let people = app.buttons[any: "People"]
        XCTAssert(people.waitForExistence(timeout: 30), "no People pane for an admin")
        people.tap()

        let more = app.buttons[any: "More for owner"]
        XCTAssert(more.waitForExistence(timeout: 10), "no menu for the account")
        more.tap()
        app.buttons[any: "Invite"].tap()

        let copy = app.buttons[any: "Copy Link"]
        XCTAssert(copy.waitForExistence(timeout: 10), "no invite sheet")
        // The list reloads once the invite is made; the sheet must outlast it.
        sleep(3)
        XCTAssert(copy.exists, "the invite sheet went away")
        let shot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        shot.name = "invite-sheet"
        shot.lifetime = .keepAlways
        add(shot)
    }
}
