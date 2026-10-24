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

/// Library items (FFI 8), the library search (FFI 9) and the shared themes (FFI 11).
final class LibraryTests: XCTestCase {
    private func study() throws -> Study {
        try Study(dataDir: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).path,
                  observer: nil)
    }

    private let rom828 = VerseRef(book: 44, chapter: 8, verse: 28)

    func testASavedNoteReadsBackThroughTheChapterAndTheList() throws {
        let study = try study()
        let uid = try study.saveNoteItem(
            text: "All things for good. See Genesis 50:20.",
            ranges: [ItemRange(from: rom828, to: VerseRef(book: 44, chapter: 8, verse: 30))])

        XCTAssertEqual(try study.chapterItems(book: 44, chapter: 8).map { [$0.first, $0.last] }, [[28, 30]])
        // The reference written in the note anchors it too.
        XCTAssertEqual(try study.chapterItems(book: 0, chapter: 50).map(\.item.uid), [uid])
        let item = try XCTUnwrap(study.libraryItem(uid: String(uid.prefix(8))))
        XCTAssertEqual(item.kind, .note)
        XCTAssertEqual(item.anchors.map(\.reference), ["Romans 8:28–30", "Genesis 50:20"])
        XCTAssertEqual(item.anchors.map(\.origin), [.user, .typed])
        XCTAssertEqual(try study.libraryItemText(uid: uid), .plain(text: "All things for good. See Genesis 50:20."))
        XCTAssertEqual(try study.libraryItems(kinds: [.sermon]), [])
        XCTAssertEqual(try study.libraryItems(kinds: nil).map(\.uid), [uid])
    }

    func testTheLibrarySearchAnswersFromTheCacheOffline() throws {
        let study = try study()
        _ = try study.saveNoteItem(text: "Joseph: what they meant for evil, God meant for good.",
                                   ranges: [ItemRange(from: rom828, to: rom828)])
        // A closed port: the server can't be reached, so the cached items' words answer.
        let sync = try LibrarySync.start(study: study, convexUrl: "http://127.0.0.1:9", tokens: Signed(), observer: nil)
        let found = try sync.searchLibrary(query: "joseph good", limit: 10)
        XCTAssertTrue(found.offline)
        XCTAssertEqual(found.hits.map(\.title), ["Joseph: what they meant for evil, God meant for good."])
        XCTAssertEqual(try sync.searchLibrary(query: "pharaoh", limit: 10).hits, [])
    }

    func testSixThemesFromTheTokenFileAndTheSyncedChoice() throws {
        XCTAssertEqual(themes(), [.linger, .paper, .forest, .oxblood, .plum, .graphite])
        XCTAssertEqual(themeLabel(theme: .paper), "Paper")
        // Linger is the desktop's design tokens exactly.
        XCTAssertEqual(themePalette(theme: .linger, dark: false).accent, 0x2D4A_9EFF)
        XCTAssertEqual(themePalette(theme: .linger, dark: true).accent, 0x8EA6_F2FF)
        XCTAssertEqual(themePalette(theme: .paper, dark: true).paper, 0x1C19_15FF)

        let study = try study()
        XCTAssertEqual(study.appearance(), Appearance(scheme: .system, theme: .linger))
        try study.setAppearance(scheme: .dark, theme: .forest)
        XCTAssertEqual(study.appearance(), Appearance(scheme: .dark, theme: .forest))
        XCTAssertEqual(study.pendingChanges(), 1, "the choice syncs")
    }
}

private final class Signed: TokenProvider, @unchecked Sendable {
    func token() -> String? { "t" }
    func refresh() -> String? { nil }
}

/// Ask (FFI 10) against a mock proxy: a tool round, then the answer.
final class AskTests: XCTestCase {
    private final class Heard: AskObserver, @unchecked Sendable {
        private let lock = NSLock()
        private var all: [AskEvent] = []
        var events: [AskEvent] { lock.withLock { all } }
        func event(event: AskEvent) { lock.withLock { all.append(event) } }
    }

    func testAnAnswerStreamsRunsAToolAndIsKeptInRecents() throws {
        let proxy = try MockProxy(replies: [
            MockProxy.sse([
                ["type": "message_start", "message": [:]],
                ["type": "content_block_start", "index": 0,
                 "content_block": ["type": "tool_use", "id": "toolu_0", "name": "get_passage", "input": [:]]],
                ["type": "content_block_delta", "index": 0,
                 "delta": ["type": "input_json_delta", "partial_json": #"{"reference":"Romans 8:28"}"#]],
                ["type": "content_block_stop", "index": 0],
                ["type": "message_delta", "delta": ["stop_reason": "tool_use"]],
                ["type": "message_stop"],
            ]),
            MockProxy.sse([
                ["type": "message_start", "message": [:]],
                ["type": "content_block_start", "index": 0, "content_block": ["type": "text", "text": ""]],
                ["type": "content_block_delta", "index": 0, "delta": ["type": "text_delta", "text": "{{Romans 8:28}} "]],
                ["type": "content_block_delta", "index": 0, "delta": ["type": "text_delta", "text": "promises good."]],
                ["type": "content_block_stop", "index": 0],
                ["type": "message_delta", "delta": ["stop_reason": "end_turn"]],
                ["type": "message_stop"],
            ]),
        ])
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).path
        let study = try Study(dataDir: dir, observer: nil)
        let bible = BibleLibrary(dataDir: dir, observer: nil)
        let conversation = Conversation(baseUrl: proxy.url, tokens: Signed())
        let heard = Heard()
        let v = VerseRef(book: 44, chapter: 8, verse: 28)
        let answer = try conversation.ask(
            bible: bible, study: study, sync: nil, question: "What does 8:28 really promise?",
            context: AskContext(book: 44, chapter: 8, translation: "kjv", selection: ItemRange(from: v, to: v), item: nil),
            scope: .bibleAndLibrary, mode: .standard, observer: heard)

        let romans828 = Reference(book: 44, chapter: 8, verse: 28, verseEnd: nil)
        XCTAssertEqual(answer.text, "{{Romans 8:28}} promises good.")
        XCTAssertEqual(answer.sources, [.passage(reference: romans828)])
        XCTAssertTrue(heard.events.contains(.read(source: .passage(reference: romans828))), "\(heard.events)")
        XCTAssertTrue(heard.events.contains(.status(label: "Reading Romans 8:28")))
        XCTAssertEqual(heard.events.compactMap { if case let .text(t) = $0 { t } else { nil } },
                       ["{{Romans 8:28}} ", "promises good."])
        XCTAssertEqual(heard.events.first, .allowance(questionsLeft: 22, downgraded: false))
        XCTAssertEqual(askSourcesLine(sources: answer.sources, done: true), "From the Bible: Romans 8:28")
        // The tool's result went back with the second round, quoting the KJV the reader shows.
        XCTAssertEqual(proxy.bodies.count, 2)
        XCTAssertTrue(proxy.bodies[1].contains("all things work together for good"))
        XCTAssertEqual(study.chats().map(\.title), ["What does 8:28 really promise?"])
        XCTAssertEqual(conversation.messages().count, 2)
    }
}

/// Answers each request with the next reply, on a socket of its own, and keeps the bodies.
private final class MockProxy: @unchecked Sendable {
    let url: String
    private let socket: Int32
    private let lock = NSLock()
    private var received: [String] = []
    var bodies: [String] { lock.withLock { received } }

    static func sse(_ events: [[String: Any]]) -> String {
        let body = events.map { e -> String in
            let json = String(data: try! JSONSerialization.data(withJSONObject: e), encoding: .utf8)!
            return "event: \(e["type"]!)\ndata: \(json)\n\n"
        }.joined()
        return "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nx-linger-questions-left: 22\r\nconnection: close\r\n\r\n" + body
    }

    init(replies: [String]) throws {
        let socket = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        self.socket = socket
        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_addr.s_addr = inet_addr("127.0.0.1")
        addr.sin_port = 0
        var len = socklen_t(MemoryLayout<sockaddr_in>.size)
        let bound = withUnsafeMutablePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(socket, $0, len) == 0 && getsockname(socket, $0, &len) == 0 }
        }
        guard bound, listen(socket, 4) == 0 else { throw URLError(.cannotConnectToHost) }
        url = "http://127.0.0.1:\(UInt16(bigEndian: addr.sin_port))"
        Thread { [self] in
            for reply in replies {
                let conn = accept(socket, nil, nil)
                guard conn >= 0 else { return }
                lock.withLock { received.append(Self.readRequest(conn)) }
                _ = reply.utf8CString.withUnsafeBufferPointer { write(conn, $0.baseAddress, $0.count - 1) }
                close(conn)
            }
        }.start()
    }

    deinit { close(socket) }

    /// The request's body: everything after the headers, up to its content-length.
    private static func readRequest(_ conn: Int32) -> String {
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 65536)
        while true {
            let n = read(conn, &buffer, buffer.count)
            guard n > 0 else { break }
            data.append(contentsOf: buffer[0..<n])
            guard let head = data.range(of: Data("\r\n\r\n".utf8)) else { continue }
            let headers = String(decoding: data[..<head.lowerBound], as: UTF8.self).lowercased()
            let length = headers.split(separator: "\r\n").first { $0.hasPrefix("content-length:") }
                .flatMap { Int($0.dropFirst("content-length:".count).trimmingCharacters(in: .whitespaces)) } ?? 0
            if data.count - head.upperBound >= length {
                return String(decoding: data[head.upperBound...], as: UTF8.self)
            }
        }
        return ""
    }
}
