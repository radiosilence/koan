import XCTest

/// Now Playing's preset choice in the theme: the theme's tray rather than a
/// system menu, a preset chosen from it and saved, and Edit… opening the EQ
/// once the tray has gone.
@MainActor
final class PresetTrayTests: XCTestCase {
    private var app: XCUIApplication!
    private var config: URL!

    override func setUp() async throws {
        continueAfterFailure = false
        addUIInterruptionMonitor(withDescription: "Notifications") { alert in
            alert.buttons.element(boundBy: 0).tap()
            return true
        }
        config = FileManager.default.temporaryDirectory.appending(path: "koan-preset-tray-\(UUID())")
        try FileManager.default.createDirectory(at: config, withIntermediateDirectories: true)
        try """
        [[dsp.profiles]]
        name = "Warm"
        role = "tuning"
        filters = [{ type = "low_shelf", freq = 105.0, gain_db = 3.0, q = 0.7 }]

        [[dsp.profiles]]
        name = "Bright Preset"
        preset = true
        layers = [{ profile = "Warm", on = true }]

        [[dsp.profiles]]
        name = "Dark Preset"
        preset = true
        layers = [{ profile = "Warm", on = false }]
        """.write(to: config.appending(path: "config.toml"), atomically: true, encoding: .utf8)
        app = XCUIApplication()
        app.launchEnvironment["KOAN_CONFIG_DIR"] = config.path
        app.launchEnvironment["KOAN_PLAYBACK__MUTED"] = "true"
        app.launchEnvironment["KOAN_PLAYBACK__RENDERERS"] = "false"
        app.launch()
    }

    override func tearDown() async throws {
        app.terminate()
        try? FileManager.default.removeItem(at: config)
    }

    func testPresetTray() throws {
        let lyrics = app.buttons[any: "Show lyrics"]
        for _ in 0..<3 where !lyrics.exists {
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.35, dy: 0.86)).tap()
            _ = lyrics.waitForExistence(timeout: 5)
        }
        XCTAssert(lyrics.exists, "Now Playing did not open")

        let preset = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "Preset")).firstMatch
        XCTAssert(preset.waitForExistence(timeout: 10), "no preset choice")
        preset.tap()
        let bright = app.buttons[any: "Bright Preset"]
        XCTAssert(bright.waitForExistence(timeout: 5), "the tray did not open")
        XCTAssert(app.buttons[any: "Flat"].exists, "no Flat")
        XCTAssert(app.buttons[any: "Edit…"].exists, "no Edit…")
        sleep(1)
        attach("tray")

        bright.tap()
        XCTAssert(bright.waitForNonExistence(timeout: 5), "choosing left the tray up")
        XCTAssert(
            app.buttons.matching(NSPredicate(format: "label CONTAINS[c] %@", "Bright Preset")).firstMatch.waitForExistence(timeout: 5),
            "the pill does not name the preset"
        )
        attach("chosen")

        preset.tap()
        let edit = app.buttons[any: "Edit…"]
        XCTAssert(edit.waitForExistence(timeout: 5), "the tray did not open again")
        XCTAssert(app.buttons[any: "Bright Preset"].isSelected, "the chosen preset is not marked")
        edit.tap()
        XCTAssert(app.buttons[any: "Done"].waitForExistence(timeout: 5), "Edit… did not open the EQ")
        sleep(1)
        attach("edit")
    }

    private func attach(_ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
