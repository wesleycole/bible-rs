import LingerCore
import Foundation
import Observation

/// What the reader shows: a translation and a chapter, loaded from linger-core off the main thread.
@MainActor
@Observable
final class ReaderModel {
    private(set) var translations: [TranslationInfo] = []
    private(set) var content: ChapterContent?
    private(set) var error: String?
    var translation = defaultTranslationId()
    /// John 1, as on the desktop: a friendlier first page than Genesis 1.
    private(set) var book: UInt8 = 42
    private(set) var chapter: UInt8 = 1

    let books = LingerCore.books()
    private let library: BibleLibrary?

    init() {
        do {
            library = try BibleLibrary.openDefault()
            translations = library?.translations() ?? []
        } catch {
            library = nil
            self.error = message(of: error)
        }
    }

    var title: String {
        Reference(book: book, chapter: chapter, verse: nil, verseEnd: nil).display
    }

    var canGoBack: Bool { book > 0 || chapter > 1 }
    var canGoForward: Bool { Int(book) < books.count - 1 || chapter < books[Int(book)].chapters }

    func go(book: UInt8, chapter: UInt8) {
        self.book = book
        self.chapter = chapter
    }

    /// Goes to a typed reference ("jn 3", "ps 23"); false if it isn't one.
    func go(to input: String) -> Bool {
        guard let r = parseReference(input: input) else { return false }
        go(book: r.book, chapter: r.chapter)
        return true
    }

    func previousChapter() {
        if chapter > 1 {
            chapter -= 1
        } else if book > 0 {
            book -= 1
            chapter = books[Int(book)].chapters
        }
    }

    func nextChapter() {
        if chapter < books[Int(book)].chapters {
            chapter += 1
        } else if Int(book) < books.count - 1 {
            book += 1
            chapter = 1
        }
    }

    /// Loads the current chapter. Decoding a translation the first time takes ~20 ms.
    func load() async {
        guard let library else { return }
        let (translation, book, chapter) = (self.translation, self.book, self.chapter)
        let result = await Task.detached(priority: .userInitiated) {
            Result { try library.chapter(translation: translation, book: book, chapter: chapter) }
        }.value
        // Ignore a result that arrives after the reader moved on.
        guard (translation, book, chapter) == (self.translation, self.book, self.chapter) else { return }
        switch result {
        case let .success(content):
            self.content = content
            self.error = nil
        case let .failure(error):
            self.content = nil
            self.error = message(of: error)
        }
    }
}
