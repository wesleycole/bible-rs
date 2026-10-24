import XCTest

/// The redesign's script on the Bible tab (docs/plans/m3-ios-app.md#m3b-the-redesign), as the
/// prototype's DemoClipTests ran it, on real chapters. Signs in first if needed.
final class ReaderTests: LingerUITestCase {
    func openBook(_ name: String) {
        XCTAssertTrue(el("book-\(name)").waitForExistence(timeout: 5), "no \(name)")
        if !el("ch-\(name)-1").exists { tap("book-\(name)") }
    }

    /// The navigator: John 3 from its book row and grid, then Romans 9 by typing "rom", then
    /// Genesis 1 through the Old Testament.
    func testNavigatorGoesToChapters() {
        tap("jump-field")
        XCTAssertTrue(el("testament").waitForExistence(timeout: 5))
        app.segmentedControls.buttons["New Testament"].tap()
        // The current book starts open; another opens on its row.
        openBook("John")
        tap("ch-John-3")
        waitForPlace("John 3")
        XCTAssertFalse(el("jump-input").exists, "the field didn't close")

        tap("jump-field")
        app.typeText("rom")
        tap("ch-Romans-9")
        waitForPlace("Romans 9")

        tap("jump-field")
        // The current chapter isn't a recent place; the one before it is.
        XCTAssertTrue(el("recent-42-3").waitForExistence(timeout: 5), "John 3 isn't in Recent")
        XCTAssertFalse(el("recent-44-9").exists)
        app.segmentedControls.buttons["Old Testament"].tap()
        openBook("Genesis")
        tap("ch-Genesis-1")
        waitForPlace("Genesis 1")
    }

    /// A typed reference goes on Return; swiping sideways turns the chapter, and the line under the
    /// last verse does too, across books; scrolling to the end doesn't; words find verses.
    func testTypingAReferenceAndPaging() {
        tap("jump-field")
        app.typeText("jn 3")
        XCTAssertTrue(app.staticTexts["Go to John 3"].waitForExistence(timeout: 5))
        app.typeText("\n")
        waitForPlace("John 3")

        app.swipeLeft()
        waitForPlace("John 4")
        app.swipeRight()
        waitForPlace("John 3")

        // Flinging past the last verse stays in the chapter.
        let next = el("next-chapter")
        for _ in 0..<12 where !next.isHittable { app.swipeUp(velocity: .fast) }
        XCTAssertEqual(next.label, "Next chapter, John 4")
        app.swipeUp(velocity: .fast)
        app.swipeUp(velocity: .fast)
        XCTAssertTrue(place.hasPrefix("John 3 ·"), "flinging up turned to \(place)")

        // The hint under the last verse turns on a tap too, here across books.
        tap("jump-field")
        app.typeText("philemon 1\n")
        waitForPlace("Philemon 1")
        for _ in 0..<12 where !next.isHittable { app.swipeUp(velocity: .fast) }
        next.tap()
        waitForPlace("Hebrews 1")

        tap("jump-field")
        app.typeText("love one another")
        tap("jump-verse-0")
        XCTAssertFalse(el("jump-input").waitForExistence(timeout: 2), "the field didn't close")
        XCTAssertTrue(place.hasPrefix("John 13 ·"), "the first match for love one another is \(place)")
    }

    /// Romans 8:28: a colour, the + anchored to it, a cancelled note keeping the selection, a saved
    /// note, ··· with Bookmark, and Ask. Leaves the verse as it found it: no colour, bookmark or note.
    func testTheVerseRow() {
        tap("jump-field")
        app.typeText("rom 8:28\n")
        waitForPlace("Romans 8")
        tap("verse-28")
        XCTAssertFalse(app.tabBars.buttons["Bible"].isHittable, "the tab bar didn't make way for the row")
        // Start clean if an earlier run left a colour.
        let yellow = el("row-dot-yellow")
        XCTAssertTrue(yellow.waitForExistence(timeout: 5), "no verse row")
        for dot in ["yellow", "green", "blue", "pink", "purple"] where el("row-dot-\(dot)").label.hasPrefix("Remove") {
            tap("row-dot-\(dot)")
        }
        tap("row-dot-yellow")
        XCTAssertEqual(yellow.label, "Remove yellow highlight")

        // The + follows the selection; cancelling a note keeps it.
        tap("plus")
        XCTAssertEqual(el("capture-anchor").label, "Anchored to Romans 8:28")
        tap("capture-note")
        tap("note-cancel")
        XCTAssertTrue(el("row-more").waitForExistence(timeout: 3), "cancelling the note dropped the selection")

        // A saved note is there when the + opens it again; an emptied one is deleted.
        tap("plus")
        tap("capture-note")
        let field = el("note-field")
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        let old = (field.value as? String) ?? ""
        app.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: old.count) + "God is good in all things.")
        tap("note-save")
        XCTAssertFalse(el("row-more").waitForExistence(timeout: 2), "saving the note kept the selection")
        tap("verse-28")
        tap("plus")
        tap("capture-note")
        XCTAssertEqual(el("note-field").value as? String, "God is good in all things.")
        app.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 26))
        tap("note-save")
        tap("verse-28")

        // ···: Bookmark turns into Remove bookmark.
        tap("row-more")
        XCTAssertTrue(app.buttons["Compare"].exists && app.buttons["Share"].exists)
        let bookmark = app.buttons["Bookmark"]
        if bookmark.exists { bookmark.tap() } else { app.buttons["Remove bookmark"].tap(); tap("row-more"); app.buttons["Bookmark"].tap() }
        tap("row-more")
        XCTAssertTrue(app.buttons["Remove bookmark"].waitForExistence(timeout: 3))
        app.buttons["Remove bookmark"].tap()
        tap("row-dot-yellow")
        XCTAssertEqual(yellow.label, "Highlight yellow")

        // Ask opens the Ask sheet with the verse quoted.
        tap("row-ask")
        XCTAssertTrue(app.staticTexts["ROMANS 8:28 · KJV"].waitForExistence(timeout: 5), "Ask didn't quote the verse")
    }

    /// ··· › Study this verse opens Romans 8:28's page: its text, its colours, a note added
    /// through the page's +, and its Ask bar. Leaves the verse without colour or note.
    func testTheVersePage() {
        tap("jump-field")
        app.typeText("rom 8:28\n")
        waitForPlace("Romans 8")
        tap("verse-28")
        tap("row-more")
        app.buttons["Study this verse"].tap()
        XCTAssertTrue(app.navigationBars["Romans 8:28"].waitForExistence(timeout: 5), "no verse page")
        XCTAssertTrue((el("page-text").label).hasPrefix("And we know that all things work together for good"))
        XCTAssertFalse(app.tabBars.buttons["Bible"].isHittable, "the tab bar shows on the verse page")
        XCTAssertFalse(el("row-more").exists, "the verse row stayed up")

        let green = el("page-dot-green")
        if green.label.hasPrefix("Remove") { green.tap() }
        green.tap()
        XCTAssertEqual(green.label, "Remove green highlight")
        green.tap()
        XCTAssertEqual(green.label, "Highlight green")

        // A note through the page's +, shown on the page, then emptied from there.
        tap("plus")
        XCTAssertEqual(el("capture-anchor").label, "Anchored to Romans 8:28")
        tap("capture-note")
        let field = el("note-field")
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        let old = (field.value as? String) ?? ""
        app.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: old.count) + "Not that all things are good.")
        tap("note-save")
        XCTAssertTrue(el("page-note-28").waitForExistence(timeout: 5), "the note isn't on the page")
        XCTAssertTrue(app.navigationBars["Romans 8:28"].exists, "saving the note left the page")
        tap("page-note-28")
        XCTAssertTrue(el("note-field").waitForExistence(timeout: 5))
        app.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 29))
        tap("note-save")
        XCTAssertFalse(el("page-note-28").waitForExistence(timeout: 2), "an emptied note stayed")

        tap("page-ask")
        XCTAssertTrue(app.staticTexts["ROMANS 8:28 · KJV"].waitForExistence(timeout: 5), "Ask didn't quote the verse")
    }

    /// A note on Romans 8:28 puts the library glyph beside it; the glyph's popover lists the note
    /// and its header opens the verse page. Empties the note again, which takes the glyph away.
    func testTheMarginPopover() {
        tap("jump-field")
        app.typeText("rom 8:28\n")
        waitForPlace("Romans 8")
        tap("verse-28")
        tap("plus")
        tap("capture-note")
        let field = el("note-field")
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        let old = (field.value as? String) ?? ""
        app.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: old.count) + "Margin popover check.")
        tap("note-save")

        tap("margin-28")
        XCTAssertTrue(el("popover-header").waitForExistence(timeout: 5), "no popover")
        XCTAssertTrue(app.staticTexts["Margin popover check."].exists, "the note isn't in the popover")
        tap("popover-header")
        XCTAssertTrue(app.navigationBars["Romans 8:28"].waitForExistence(timeout: 5), "the header didn't open the verse page")

        tap("page-note-28")
        XCTAssertTrue(el("note-field").waitForExistence(timeout: 5))
        app.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 21))
        tap("note-save")
        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(el("verse-28").waitForExistence(timeout: 5))
        XCTAssertFalse(el("margin-28").waitForExistence(timeout: 3), "the glyph stayed after the note went")
    }

    /// × closes the field without moving.
    func testClosingTheFieldStays() {
        let before = place
        tap("jump-field")
        tap("jump-close")
        XCTAssertTrue(el("jump-field").waitForExistence(timeout: 3))
        XCTAssertEqual(place, before)
    }
}
