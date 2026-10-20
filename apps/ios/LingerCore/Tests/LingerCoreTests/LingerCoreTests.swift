import LingerCore
import Foundation
import XCTest

/// Exercises the Rust library through the generated bindings: records, enums with fields,
/// errors and optionals all cross the boundary.
final class LingerCoreTests: XCTestCase {
    private func library() -> BibleLibrary {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        return BibleLibrary(dataDir: dir.path, observer: nil)
    }

    func testListsTheBundledTranslations() {
        let ids = library().translations().map(\.id)
        XCTAssertEqual(Set(ids), ["kjv", "web", "bsb", "asv", "ylt", "dby", "wbs"])
        XCTAssertEqual(defaultTranslationId(), "kjv")
    }

    func testReadsJohn3() throws {
        guard case let .ready(verses, notice) = try library().chapter(translation: "kjv", book: 42, chapter: 3) else {
            return XCTFail("John 3 should be ready")
        }
        XCTAssertEqual(verses.count, 36)
        XCTAssertEqual(verses[15].number, 16)
        XCTAssertTrue(verses[15].text.contains("only begotten Son"))
        XCTAssertNil(notice)
    }

    func testThrowsForAChapterOutsideTheCanon() {
        XCTAssertThrowsError(try library().chapter(translation: "kjv", book: 64, chapter: 2)) { error in
            XCTAssertEqual(error as? BibleError, .InvalidChapter(book: 64, chapter: 2))
        }
    }

    func testSearchesAndParsesReferences() throws {
        let results = try library().search(
            translation: "kjv", query: "\"jesus wept\"", scope: .all, wholeWords: false, limit: 5)
        XCTAssertEqual(results.total, 1)
        XCTAssertEqual(results.hits.first?.reference.display, "John 11:35")

        let r = try XCTUnwrap(parseReference(input: "revalation 21"))
        XCTAssertEqual(r.book, 65)
        XCTAssertEqual(r.display, "Revelation 21")
        XCTAssertEqual(books().count, 66)
    }
}

/// Study data and sync through the bindings, with observers called back from Rust threads.
final class StudyTests: XCTestCase {
    private final class Observer: StudyObserver, SyncObserver, @unchecked Sendable {
        let changed: XCTestExpectation
        private(set) var generation: UInt64 = 0
        init(_ changed: XCTestExpectation) { self.changed = changed }
        func studyChanged(generation: UInt64) {
            self.generation = generation
            changed.fulfill()
        }
        func syncChanged() {}
    }

    private final class Tokens: TokenProvider, @unchecked Sendable {
        let asked: XCTestExpectation
        init(_ asked: XCTestExpectation) { self.asked = asked }
        func token() -> String? {
            asked.fulfill()
            return nil
        }
        func refresh() -> String? { nil }
    }

    private func dataDir() -> String {
        FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).path
    }

    func testANoteWrittenThroughStudyReachesTheObserver() throws {
        let changed = expectation(description: "the observer is told")
        changed.assertForOverFulfill = false
        let observer = Observer(changed)
        let study = try Study(dataDir: dataDir(), observer: observer)
        let verse = VerseRef(book: 42, chapter: 3, verse: 16)

        try study.setNote(verse: verse, body: "For God so loved")
        try study.setHighlight(verses: [verse], color: .green)
        wait(for: [changed], timeout: 1)

        XCTAssertGreaterThan(observer.generation, 0)
        XCTAssertEqual(try study.note(verse: verse)?.body, "For God so loved")
        XCTAssertEqual(
            try study.chapterMarks(book: 42, chapter: 3),
            [VerseMarks(verse: 16, highlight: .green, note: true, bookmark: false)])
        XCTAssertEqual(study.notes().map(\.reference), ["John 3:16"])
        XCTAssertEqual(highlightColors().count, 6)
        XCTAssertEqual(study.pendingChanges(), 2)
    }

    func testATokenProviderFeedsSync() throws {
        let asked = expectation(description: "sync asks for a token")
        asked.assertForOverFulfill = false
        let study = try Study(dataDir: dataDir(), observer: nil)
        // A closed port: the engine asks for a token before it ever reaches the server.
        let sync = try LibrarySync.start(
            study: study, convexUrl: "http://127.0.0.1:9", tokens: Tokens(asked), observer: nil)
        wait(for: [asked], timeout: 5)
        XCTAssertNotEqual(sync.status(), .synced)
    }

    func testSplitsTextAtReferences() {
        let segments = referenceSegments(text: "See Rom 8:28.")
        XCTAssertEqual(segments.count, 3)
        guard case let .reference(text, reference) = segments[1] else { return XCTFail("\(segments)") }
        XCTAssertEqual(text, "Rom 8:28")
        XCTAssertEqual(reference.display, "Romans 8:28")
    }
}
