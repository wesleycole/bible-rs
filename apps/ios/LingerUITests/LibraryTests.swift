import XCTest

/// Library from the cache: the reader's "N in your library" opens it on the chapter, the jump
/// field finds a note's words, and an item's page asks about it. Needs the test user's library
/// synced (it holds sermons recorded on the Mac); leaves Romans 8:28 without a note.
final class LibraryTests: LingerUITestCase {
    /// A note on Romans 8:28 adds one to "N in your library" under the chapter, which opens Library
    /// on Romans 8 with the note; the field's words find it under "In your library".
    func testTheChapterLineAndTheField() {
        // The test user's library may already hold items on Romans 8, from other devices.
        go("rom 8", expect: "Romans 8")
        let line = el("chapter-library")
        let before = line.waitForExistence(timeout: 2) ? Int(line.label.prefix { $0.isNumber }) ?? 0 : 0
        go("rom 8:28", expect: "Romans 8")
        setNoteOnRomans828("Groanings that cannot be uttered")
        // Going to the chapter brings its top into view.
        go("rom 8", expect: "Romans 8")
        XCTAssertTrue(line.waitForExistence(timeout: 5), "no line under the chapter")
        XCTAssertTrue(line.label.hasPrefix("\(before + 1) in your library"), "the line reads \(line.label)")
        shot("Bible, reading")
        line.tap()
        XCTAssertTrue(el("filter-Romans 8").waitForExistence(timeout: 5), "Library didn't open on Romans 8")
        XCTAssertTrue(el("filter-Romans 8").isSelected)
        let row = app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH 'Groanings that cannot be uttered'")).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 5), "the note isn't listed on Romans 8")
        tap("filter-All")

        // The field's words: verse matches, then the library's.
        app.tabBars.buttons["Bible"].tap()
        tap("jump-field")
        app.typeText("groanings uttered")
        // The server indexes the note as it syncs; search again once if it hadn't yet.
        if !el("jump-item-0").waitForExistence(timeout: 8) {
            app.typeText(XCUIKeyboardKey.delete.rawValue + "d")
        }
        XCTAssertTrue(el("jump-item-0").waitForExistence(timeout: 10), "the note isn't under In your library")
        shot("Bible, the field's library results")
        tap("jump-item-0")
        XCTAssertTrue(el("item-title").waitForExistence(timeout: 5), "the result didn't open the note's page")
        XCTAssertEqual(el("item-text").label, "Groanings that cannot be uttered")
        // Back on the Bible tab, the note goes again.
        app.navigationBars.buttons.element(boundBy: 0).tap()
        app.tabBars.buttons["Bible"].tap()
        go("rom 8:28", expect: "Romans 8")
        setNoteOnRomans828("")
        if before == 0 {
            XCTAssertFalse(line.waitForExistence(timeout: 3), "the line stayed after the note went")
        } else {
            XCTAssertTrue(line.label.hasPrefix("\(before) in your library"), "the note still counts after it went")
        }
    }

    /// Library's home lists the sermons; a sermon's page has its passages and transcript, and its
    /// Ask bar opens Ask about it.
    func testASermonsPage() {
        app.tabBars.buttons["Library"].tap()
        XCTAssertTrue(el("library-count").waitForExistence(timeout: 5))
        XCTAssertTrue(el("library-note").exists, "no Note in the capture row")
        for hidden in ["Record", "Photo", "Import", "Yours", "Reference"] {
            XCTAssertFalse(app.buttons[hidden].exists, "\(hidden) shows before its milestone")
        }
        shot("Library home")
        tap("filter-Sermons")
        let sermon = app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH 'item-'")).firstMatch
        XCTAssertTrue(sermon.waitForExistence(timeout: 5), "no sermons in the library")
        sermon.tap()
        XCTAssertTrue(el("item-title").waitForExistence(timeout: 5), "no item page")
        XCTAssertTrue(el("item-share").exists, "no Share")
        XCTAssertFalse(app.tabBars.buttons["Library"].isHittable, "the tab bar shows on the item page")
        shot("Library sermon page")
        // Its passages open in Bible, and its transcript is timed text.
        XCTAssertTrue(app.staticTexts["Passages"].exists, "no passages")
        app.swipeUp()
        XCTAssertTrue(app.staticTexts["Transcript"].waitForExistence(timeout: 3), "no transcript")
        shot("Library sermon page, transcript")
        tap("item-ask")
        let field = el("ask-field")
        XCTAssertTrue(field.waitForExistence(timeout: 5), "Ask didn't open")
        XCTAssertEqual(field.placeholderValue, "Ask about this sermon")
        tap("ask-close")
        XCTAssertTrue(el("item-title").waitForExistence(timeout: 5), "closing Ask left the page")
    }
}
