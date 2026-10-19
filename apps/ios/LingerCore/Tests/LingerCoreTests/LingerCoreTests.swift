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
