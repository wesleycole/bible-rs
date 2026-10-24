import XCTest

/// Ask against the proxy from `pnpm dev` on the Mac (127.0.0.1:3000), as the test user: a
/// question about Romans 8:28 streams an answer, a source opens in Bible, and the return bar
/// brings the chat back as it was.
final class AskTests: LingerUITestCase {
    /// The Ask slot asks about what's on screen and leaves the tab where it was.
    func testTheAskSlot() {
        go("rom 8", expect: "Romans 8")
        app.tabBars.buttons["Ask"].tap()
        XCTAssertTrue(app.staticTexts["Reading Romans 8"].waitForExistence(timeout: 5), "Ask isn't about the chapter")
        tap("ask-history")
        XCTAssertTrue(app.navigationBars["History"].waitForExistence(timeout: 5), "no history")
        app.navigationBars["History"].buttons["Done"].tap()
        tap("ask-close")
        XCTAssertTrue(el("jump-field").waitForExistence(timeout: 5), "closing Ask didn't leave Bible on screen")
        XCTAssertTrue(app.tabBars.buttons["Bible"].isSelected)
    }

    func testAskingAboutAVerse() {
        go("rom 8:28", expect: "Romans 8")
        tap("verse-28")
        shot("Bible, verses selected")
        tap("row-ask")
        XCTAssertTrue(app.staticTexts["ROMANS 8:28 · KJV"].waitForExistence(timeout: 5), "Ask didn't quote the verse")
        XCTAssertTrue(el("ask-suggestion-0").waitForExistence(timeout: 5), "no suggested questions")
        XCTAssertEqual(el("ask-field").placeholderValue, "Ask about Romans 8:28")
        shot("Ask, before asking")

        let question = "What does this verse promise? Answer in three short paragraphs and cite it."
        el("ask-field").tap()
        app.typeText(question)
        tap("ask-send")
        let consent = el("ask-consent-continue")
        if consent.waitForExistence(timeout: 3) { consent.tap() }
        XCTAssertTrue(app.staticTexts[question].waitForExistence(timeout: 5), "the question isn't in the chat")
        // The send button stops while the answer is written; a fast answer can be done first.
        let send = el("ask-send")
        let writing = NSPredicate { _, _ in
            send.label == "Stop" && (self.el("ask-answer").exists || self.el("ask-reading").exists)
        }
        if XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: writing, object: nil)], timeout: 20) == .completed {
            shot("Ask, while answering")
        }

        let sources = el("ask-sources")
        XCTAssertTrue(sources.waitForExistence(timeout: 120), "the answer didn't finish with its sources line")
        // "Read 1 sermon from your library", or "From the Bible: Romans 8:28" without the library.
        XCTAssertTrue(sources.label.hasPrefix("Read") || sources.label.hasPrefix("From the Bible"), "the line reads \(sources.label)")
        XCTAssertTrue(el("ask-save").exists && el("ask-share").exists, "no answer actions")
        XCTAssertEqual(el("ask-field").placeholderValue, "Ask a follow-up")
        let answer = el("ask-answer").label
        shot("Ask, after the answer")

        // The sources unfold; one peeks, and Open in Bible leaves the return bar.
        sources.tap()
        tap("ask-source-bible-0")
        XCTAssertTrue(app.buttons["Open in Bible"].waitForExistence(timeout: 5), "no peek card")
        shot("Ask, a source's peek card")
        tap("peek-open")
        let back = el("return-bar")
        XCTAssertTrue(back.waitForExistence(timeout: 5), "no return bar")
        XCTAssertEqual(back.label, "Back to \(question)")
        shot("Ask, a source open with the return bar")
        back.tap()
        XCTAssertTrue(el("ask-answer").waitForExistence(timeout: 5), "the return bar didn't bring the chat back")
        XCTAssertEqual(el("ask-answer").label, answer)
        tap("ask-close")
        XCTAssertFalse(back.waitForExistence(timeout: 2), "the return bar stayed after closing Ask")
    }
}
