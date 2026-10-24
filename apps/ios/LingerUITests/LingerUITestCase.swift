import XCTest

/// Launches the app signed in on the reader, with the lookups the redesign's tests share.
class LingerUITestCase: XCTestCase {
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
        // The margin glyph sits partly past the screen's edge, which XCUITest calls not hittable.
        if e.isHittable { e.tap() } else { e.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap() }
    }

    /// The field's text, "John 3 · KJV".
    var place: String { el("jump-field").label }

    func waitForPlace(_ prefix: String) {
        let shown = NSPredicate { _, _ in self.place.hasPrefix(prefix + " ·") }
        let wait = XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: shown, object: nil)], timeout: 8)
        XCTAssertEqual(wait, .completed, "the field reads \(place), not \(prefix)")
    }

    /// Types a reference into the jump field and goes there.
    func go(_ reference: String, expect prefix: String) {
        tap("jump-field")
        app.typeText(reference + "\n")
        waitForPlace(prefix)
    }

    /// Writes (or with "" empties) Romans 8:28's note through the toolbar +, with the verse
    /// selected.
    func setNoteOnRomans828(_ text: String) {
        tap("verse-28")
        tap("plus")
        tap("capture-note")
        let field = el("note-field")
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        let old = (field.value as? String) ?? ""
        let placeholder = old == "Write a note…"
        app.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: placeholder ? 0 : old.count) + text)
        tap("note-save")
    }

    /// Keeps a screenshot with the test's results, named for the screen it shows.
    func shot(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
