import LingerCore
import Foundation
import Observation
import SwiftUI

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

    /// Chapters read before this one, newest first, for the navigator.
    private(set) var recents: [Reference] = ReaderModel.savedRecents()
    /// The verse to bring into view once its chapter shows; 0 is the top of the chapter.
    var scrollTarget: UInt8?
    /// The verse a reference jump landed on, tinted until the next tap.
    var tinted: UInt8?
    /// The jump field is open over the reader; `jumpExpanded` once it has grown to full width.
    private(set) var jumpOpen = false
    var jumpExpanded = false
    /// Where the closed field sits, so the open one grows from there.
    @ObservationIgnored var jumpFieldFrame: CGRect = .zero

    let books = LingerCore.books()
    private let library: BibleLibrary?
    /// Which chapter `content` holds: it keeps the last one while the next loads.
    private var shown: (String, UInt8, UInt8)?

    init() {
        do {
            library = try BibleLibrary.openDefault()
            translations = library?.translations() ?? []
        } catch {
            library = nil
            self.error = message(of: error)
        }
    }

    var place: Reference { Reference(book: book, chapter: chapter, verse: nil, verseEnd: nil) }

    /// "Romans 8".
    var title: String { place.display }

    var translationAbbrev: String {
        translations.first { $0.id == translation }?.abbrev ?? translation.uppercased()
    }

    /// The chapter before this one, across books; nil at Genesis 1.
    var previous: Reference? {
        if chapter > 1 { return Reference(book: book, chapter: chapter - 1, verse: nil, verseEnd: nil) }
        guard book > 0 else { return nil }
        return Reference(book: book - 1, chapter: books[Int(book) - 1].chapters, verse: nil, verseEnd: nil)
    }

    /// The chapter after this one, across books; nil at Revelation 22.
    var next: Reference? {
        if chapter < books[Int(book)].chapters { return Reference(book: book, chapter: chapter + 1, verse: nil, verseEnd: nil) }
        guard Int(book) < books.count - 1 else { return nil }
        return Reference(book: book + 1, chapter: 1, verse: nil, verseEnd: nil)
    }

    /// Goes to a chapter, or to a verse in it, which is tinted. The chapter left joins Recents.
    func go(book: UInt8, chapter: UInt8, verse: UInt8? = nil) {
        if (book, chapter) != (self.book, self.chapter) {
            recents.removeAll { ($0.book, $0.chapter) == (book, chapter) || ($0.book, $0.chapter) == (self.book, self.chapter) }
            recents.insert(place, at: 0)
            recents = Array(recents.prefix(5))
            UserDefaults.standard.set(recents.map { [Int($0.book), Int($0.chapter)] }, forKey: "recentPlaces")
        }
        self.book = book
        self.chapter = chapter
        tinted = verse
        scrollTarget = verse ?? 0
    }

    func openJump() {
        jumpOpen = true
    }

    /// Shrinks the open field back into the bar, then removes it.
    func closeJump() {
        guard jumpOpen else { return }
        withAnimation(.easeIn(duration: 0.2)) { jumpExpanded = false }
        Task {
            try? await Task.sleep(for: .milliseconds(220))
            if !jumpExpanded { jumpOpen = false }
        }
    }

    private static func savedRecents() -> [Reference] {
        let saved = UserDefaults.standard.array(forKey: "recentPlaces") as? [[Int]] ?? []
        return saved.compactMap { pair in
            guard pair.count == 2, let book = UInt8(exactly: pair[0]), let chapter = UInt8(exactly: pair[1]) else { return nil }
            return Reference(book: book, chapter: chapter, verse: nil, verseEnd: nil)
        }
    }

    /// True once `content` is the chapter on screen, so a scroll target can be found in it.
    var isShowingCurrent: Bool {
        shown.map { $0 == (translation, book, chapter) } ?? false
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
        shown = (translation, book, chapter)
    }

    /// Verses in the current translation matching typed words, in canonical order; empty for an
    /// online translation, which can't be searched.
    func search(_ query: String) async -> [SearchHit] {
        guard let library else { return [] }
        let translation = self.translation
        return await Task.detached(priority: .userInitiated) {
            (try? library.search(translation: translation, query: query, scope: .all, wholeWords: false, limit: 50))?.hits ?? []
        }.value
    }
}
