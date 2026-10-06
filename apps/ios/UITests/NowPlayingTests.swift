import XCTest

/// Now Playing's rows, opened and used: the sleep timer set, the preset menu,
/// the Output and Control sheets, and with `KOAN_SHOTS_CONTROL` naming a
/// device, controlling it. Keeps a screenshot of each step, like `WalkTests`;
/// what is worth checking is how the rows look at a device's width.
@MainActor
final class NowPlayingTests: XCTestCase {
    private var app: XCUIApplication!
    private let env = ProcessInfo.processInfo.environment

    override func setUp() async throws {
        continueAfterFailure = true
        app = XCUIApplication()
        // Silent, and blind to UPnP renderers: tests share a machine with its owner.
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        app.launch()
    }

    func testRows() {
        pause(Double(env["KOAN_WALK_SETTLE"] ?? "3") ?? 3)
        // This phone again, should a run before have left it controlling
        // another device: a track tapped would be sent there.
        controlThisPhone()
        // The queue the app restored, played from Now Playing.
        openNowPlaying()
        let play = onScreen(app.buttons.matching(NSPredicate(format: "label == %@", "Play")))
        if play.exists { play.tap() }
        _ = app.buttons["Pause"].firstMatch.waitForExistence(timeout: 30)
        pause(3)
        snap("01-now-playing")

        if open(labelled(beginningWith: "Sleep timer")), open(app.buttons["30 Minutes"]) {
            pause(2)
            snap("02-sleep-timer")
        }

        // The preset is chosen from its menu, as a listener would, and the
        // menu opened again to see the names it offers.
        if let preset = env["KOAN_SHOTS_PRESET"], open(labelled(beginningWith: "Preset")) {
            pause(1)
            if open(app.buttons[preset].firstMatch) { pause(2) }
            snap("03-preset-chosen")
            if open(labelled(beginningWith: "Preset")) {
                pause(1)
                snap("03b-preset-menu")
                dismissMenu()
            }
        }

        if open(labelled(beginningWith: "Output")) {
            pause(2)
            snap("04-output-sheet")
            dismissSheet()
        }

        if open(labelled(beginningWith: "Control")) {
            pause(2)
            snap("05-control-sheet")
            if let device = env["KOAN_SHOTS_CONTROL"], open(labelled(containing: device)) {
                pause(4)
                dismissSheet()
                pause(2)
                snap("06-controlling")
                if open(labelled(beginningWith: "Sleep timer")), open(app.buttons["End of Record"]) {
                    pause(2)
                    snap("07-controlling-sleep")
                }
                if open(labelled(beginningWith: "Output")) {
                    pause(2)
                    snap("08-controlled-output-sheet")
                    dismissSheet()
                }
                if open(labelled(beginningWith: "Sleep timer")), open(app.buttons["Cancel Sleep Timer"]) {
                    pause(1)
                }
                controlThisPhone()
            } else {
                dismissSheet()
            }
        }
    }

    private func controlThisPhone() {
        let controlling = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "Controlling")).firstMatch
        guard controlling.waitForExistence(timeout: 2) else { return }
        onScreen(app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "Controlling"))).tap()
        pause(2)
        if open(labelled(containing: "This \(UIDevice.current.model)")) { pause(2) }
        dismissSheet()
    }

    /// The mini player: above the tab bar on a phone, at the foot on an iPad.
    private func openNowPlaying() {
        let pad = UIDevice.current.userInterfaceIdiom == .pad
        // A tap while the app is busy can be lost; tried until the sheet is up.
        for _ in 0..<3 where !app.buttons["Show lyrics"].exists {
            app.coordinate(withNormalizedOffset: CGVector(dx: pad ? 0.2 : 0.35, dy: pad ? 0.965 : 0.86)).tap()
            _ = app.buttons["Show lyrics"].waitForExistence(timeout: 5)
        }
        pause(2)
    }

    /// The one on screen: the mini player's buttons share their labels with
    /// Now Playing's, and stay in the tree behind its sheet.
    private func labelled(beginningWith prefix: String) -> XCUIElement {
        onScreen(app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", prefix)))
    }

    private func labelled(containing text: String) -> XCUIElement {
        onScreen(app.buttons.matching(NSPredicate(format: "label CONTAINS %@", text)))
    }

    private func onScreen(_ query: XCUIElementQuery) -> XCUIElement {
        _ = query.firstMatch.waitForExistence(timeout: 3)
        return query.allElementsBoundByIndex.last { $0.isHittable } ?? query.firstMatch
    }

    private func open(_ element: XCUIElement) -> Bool {
        guard element.waitForExistence(timeout: 3) else { return false }
        element.tap()
        return true
    }

    private func dismissSheet() {
        app.swipeDown(velocity: .fast)
        pause(1)
    }

    /// A tap above the menu, on the sleeve, closes it without choosing.
    private func dismissMenu() {
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.15)).tap()
        pause(1)
    }

    private func pause(_ seconds: TimeInterval) {
        _ = XCTWaiter.wait(for: [expectation(description: "settle")], timeout: seconds)
    }

    private func snap(_ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
