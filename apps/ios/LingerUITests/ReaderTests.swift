import XCTest

/// The redesign's script on the Bible tab (docs/plans/m3-ios-app.md#m3b-the-redesign), as the
/// prototype's DemoClipTests ran it, on real chapters. Signs in first if needed.
final class ReaderTests: XCTestCase {
    var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
        app = XCUIApplication()
        app.launch()
        app.signIn()
    }

    func el(_ id: String) -> XCUIElement { app.descendants(matching: .any)[id].firstMatch }

    func tap(_ id: String, timeout: TimeInterval = 8) {
        let e = el(id)
        XCTAssertTrue(e.waitForExistence(timeout: timeout), "no \(id)")
        e.tap()
    }

    /// The field's text, "John 3 · KJV".
    var place: String { el("jump-field").label }

    func waitForPlace(_ prefix: String) {
        let shown = NSPredicate { _, _ in self.place.hasPrefix(prefix + " ·") }
        let wait = XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: shown, object: nil)], timeout: 8)
        XCTAssertEqual(wait, .completed, "the field reads \(place), not \(prefix)")
    }

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

    /// A typed reference goes on Return; the chapter's end links to the next one; words find verses.
    func testTypingAReferenceAndPaging() {
        tap("jump-field")
        app.typeText("jn 3")
        XCTAssertTrue(app.staticTexts["Go to John 3"].waitForExistence(timeout: 5))
        app.typeText("\n")
        waitForPlace("John 3")

        let next = el("next-chapter")
        for _ in 0..<12 where !next.isHittable { app.swipeUp(velocity: .fast) }
        next.tap()
        waitForPlace("John 4")
        tap("previous-chapter")
        waitForPlace("John 3")

        tap("jump-field")
        app.typeText("love one another")
        tap("jump-verse-0")
        XCTAssertFalse(el("jump-input").waitForExistence(timeout: 2), "the field didn't close")
        XCTAssertTrue(place.hasPrefix("John 13 ·"), "the first match for love one another is \(place)")
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
