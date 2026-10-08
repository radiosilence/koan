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
        // One track to play, so Now Playing has something to open on.
        let music = config.appending(path: "music")
        try FileManager.default.createDirectory(at: music, withIntermediateDirectories: true)
        try Self.wav().write(to: music.appending(path: "tone.wav"))
        try """
        [library]
        folders = ["\(music.path)"]

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
        let search = app.buttons[any: "Search"]
        XCTAssert(search.waitForExistence(timeout: 10), "no Search tab")
        // The row, not the heading over the results, which names the search too.
        let track = app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH[c] %@", "Quiet Tone")).firstMatch
        search.tap()
        var field = app.searchFields.firstMatch
        if !field.waitForExistence(timeout: 5) { field = app.textFields[any: "Artists, albums, tracks"] }
        field.tap()
        field.typeText("Quiet Tone")
        // The startup scan comes after the first frame: the search is run
        // again, a keystroke at a time, until the track is in it.
        for _ in 0..<10 where !track.waitForExistence(timeout: 3) {
            field.typeText(" " + XCUIKeyboardKey.delete.rawValue)
        }
        XCTAssert(track.exists, "the track was never indexed")
        // The keyboard away first: it covers the mini player.
        field.typeText("\n")
        track.tap()
        sleep(2)
        attach("played")

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

    /// Half a minute of silence, tagged so a search finds it.
    private static func wav() -> Data {
        func chunk(_ id: String, _ body: Data) -> Data {
            var d = Data(id.utf8)
            d.append(le(UInt32(body.count)))
            d.append(body)
            if body.count % 2 == 1 { d.append(0) }
            return d
        }
        func le<T: FixedWidthInteger>(_ v: T) -> Data { withUnsafeBytes(of: v.littleEndian) { Data($0) } }
        func text(_ s: String) -> Data { Data(s.utf8) + Data([0]) }
        let rate: UInt32 = 44_100
        var fmt = Data()
        fmt.append(le(UInt16(1))); fmt.append(le(UInt16(2))); fmt.append(le(rate))
        fmt.append(le(rate * 4)); fmt.append(le(UInt16(4))); fmt.append(le(UInt16(16)))
        var info = Data("INFO".utf8)
        info.append(chunk("INAM", text("Quiet Tone")))
        info.append(chunk("IART", text("Quiet Artist")))
        info.append(chunk("IPRD", text("Quiet Album")))
        var body = Data("WAVE".utf8)
        body.append(chunk("fmt ", fmt))
        body.append(chunk("LIST", info))
        body.append(chunk("data", Data(count: Int(rate) * 4 * 30)))
        return chunk("RIFF", body)
    }

    private func attach(_ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
