import LingerCore
import Observation
import SwiftUI

/// What a question is about: the selected verses, the chapter being read, or a library item.
struct AskAbout: Equatable {
    var book: UInt8
    var chapter: UInt8
    var translation: String
    var translationAbbrev: String
    var selection: ClosedRange<UInt8>?
    /// The selected verses' text, quoted at the top of the sheet.
    var quote = ""
    var item: LingerCore.LibraryItem?

    var place: Reference { Reference(book: book, chapter: chapter, verse: nil, verseEnd: nil) }

    /// "Romans 8:28" or "Romans 8:26–28".
    var selectionTitle: String? {
        selection.map {
            Reference(book: book, chapter: chapter, verse: $0.lowerBound, verseEnd: $0.count > 1 ? $0.upperBound : nil).display
        }
    }

    var context: AskContext {
        AskContext(book: book, chapter: chapter, translation: translation,
                   selection: selection.map {
                       ItemRange(from: VerseRef(book: book, chapter: chapter, verse: $0.lowerBound),
                                 to: VerseRef(book: book, chapter: chapter, verse: $0.upperBound))
                   },
                   item: item?.uid)
    }
}

/// One question and its answer as the chat shows it.
struct AskTurn: Identifiable {
    enum State: Equatable { case answering, done, stopped }

    let id = UUID()
    let question: String
    var text = ""
    /// What it's doing while it answers: "Reading Romans 8:28".
    var status: String?
    /// What its tools have read so far.
    var read: [AskSource] = []
    /// Once done: what it read, then what it cites.
    var sources: [AskSource] = []
    var state = State.answering
    var saved = false

    /// The grey line under the answer: what it is reading, then what it read.
    var sourcesLine: String? {
        switch state {
        // Once the text streams with nothing read from the library, the answer speaks for itself.
        case .answering: askSourcesLine(sources: read, done: false) ?? status.map { "\($0)…" } ?? (text.isEmpty ? "Thinking…" : nil)
        case .done: askSourcesLine(sources: sources, done: true)
        case .stopped: "Stopped"
        }
    }
}

/// Why a question got no answer, and whether Pro would help.
struct AskNotice: Equatable {
    let message: String
    var pro = false
}

/// The Ask sheet's state: its context and scope, the conversation and its answers as they stream.
@MainActor
@Observable
final class AskModel {
    /// The sheet is up.
    var presented = false
    private(set) var about: AskAbout?
    var scope = AskScope.bibleAndLibrary
    private(set) var turns: [AskTurn] = []
    private(set) var notice: AskNotice?
    /// Questions left this month, as the proxy said with the last answer.
    private(set) var questionsLeft: UInt32?
    /// The question the user left the chat to read a source about: the return bar's label.
    private(set) var leftFrom: String?
    /// The question waiting in the composer.
    var draft = ""
    /// The Ask consent sheet's answer, asked before the first question.
    var consented = UserDefaults.standard.bool(forKey: "askConsent") {
        didSet { UserDefaults.standard.set(consented, forKey: "askConsent") }
    }

    private var conversation = Conversation(baseUrl: Config.apiURL, tokens: ClerkTokens())
    private let library: Library
    private let bible: BibleLibrary?

    init(library: Library, bible: BibleLibrary?) {
        self.library = library
        self.bible = bible
    }

    var answering: Bool { turns.last?.state == .answering }

    /// Out of questions: the composer is disabled, with a link to Pro.
    var spent: Bool { questionsLeft == 0 }

    /// Three questions drawn from the context and the library, while nothing has been asked.
    var suggestions: [String] {
        guard let about, let study = library.study, turns.isEmpty else { return [] }
        _ = library.generation
        return askSuggestions(study: study, context: about.context, scope: scope)
    }

    /// "Ask about Romans 8:28", "Ask about Romans 8", "Ask about this sermon"; "Ask a follow-up".
    var placeholder: String {
        if answering { return "Answering" }
        if !turns.isEmpty { return "Ask a follow-up" }
        if let item = about?.item { return "Ask about this \(item.kind.noun)" }
        return "Ask about \(about?.selectionTitle ?? about?.place.display ?? "the Bible")"
    }

    /// A new question about what's on screen: the Ask button, a verse row, the popover, a page.
    func start(_ about: AskAbout) {
        stopIfAnswering()
        self.about = about
        scope = .bibleAndLibrary
        turns = []
        notice = nil
        draft = ""
        leftFrom = nil
        conversation = Conversation(baseUrl: Config.apiURL, tokens: ClerkTokens())
        presented = true
    }

    /// A past chat from history, to read and carry on.
    func open(chat id: Int64, about: AskAbout) {
        guard let study = library.study else { return }
        do {
            let opened = try Conversation.open(study: study, id: id, baseUrl: Config.apiURL, tokens: ClerkTokens())
            stopIfAnswering()
            conversation = opened
            self.about = about
            notice = nil
            leftFrom = nil
            var turns: [AskTurn] = []
            for message in opened.messages() {
                if message.fromUser {
                    turns.append(AskTurn(question: message.text))
                } else if !turns.isEmpty {
                    turns[turns.count - 1].text = message.text
                    turns[turns.count - 1].state = .done
                    turns[turns.count - 1].sources = Self.cited(message.text)
                }
            }
            self.turns = turns.filter { $0.state == .done }
            presented = true
        } catch {
            notice = AskNotice(message: message(of: error))
        }
    }

    /// The × on the quoted selection: the question is about the chapter instead.
    func dropSelection() {
        about?.selection = nil
        about?.quote = ""
    }

    func send(_ question: String? = nil) {
        let question = (question ?? draft).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !question.isEmpty, !answering, !spent, let about, let study = library.study, let bible else { return }
        draft = ""
        notice = nil
        turns.append(AskTurn(question: question))
        let (conversation, sync, scope, relay) = (conversation, library.librarySync, scope, Relay(model: self))
        Task {
            let result = await Task.detached(priority: .userInitiated) {
                Result {
                    try conversation.ask(bible: bible, study: study, sync: sync, question: question,
                                         context: about.context, scope: scope, mode: .standard, observer: relay)
                }
            }.value
            finish(result, question: question)
        }
    }

    /// The send button while an answer is written.
    func stop() {
        conversation.stop()
    }

    /// Save to library: the answer as a note, anchored to the verses it cites.
    func save(_ turn: AskTurn) {
        let ranges = turn.sources.compactMap { source -> ItemRange? in
            guard case let .passage(r) = source, let verse = r.verse else { return nil }
            return ItemRange(from: VerseRef(book: r.book, chapter: r.chapter, verse: verse),
                             to: VerseRef(book: r.book, chapter: r.chapter, verse: r.verseEnd ?? verse))
        }
        library.write { _ = try $0.saveNoteItem(text: turn.text, ranges: ranges) }
        if let i = turns.firstIndex(where: { $0.id == turn.id }) { turns[i].saved = true }
    }

    /// Opening a source collapses the sheet and leaves the return bar.
    func leave() {
        leftFrom = turns.last?.question
        presented = false
    }

    /// The return bar: the chat as it was.
    func comeBack() {
        presented = true
    }

    /// The sheet was swiped away or closed with ×: no return bar.
    func close() {
        leftFrom = nil
        presented = false
    }

    private func stopIfAnswering() {
        if answering { conversation.stop() }
    }

    fileprivate func heard(_ event: AskEvent) {
        guard let i = turns.indices.last, turns[i].state == .answering else { return }
        switch event {
        case let .text(text):
            turns[i].text += text
            turns[i].status = nil
        case let .status(label):
            turns[i].status = label
        case let .read(source):
            turns[i].read.append(source)
        case let .allowance(left, _):
            if let left { questionsLeft = left }
        }
    }

    private func finish(_ result: Result<AskAnswer, Error>, question: String) {
        guard let i = turns.lastIndex(where: { $0.question == question && $0.state == .answering }) else { return }
        switch result {
        case let .success(answer):
            turns[i].text = answer.text
            turns[i].sources = answer.sources
            turns[i].status = nil
            turns[i].state = .done
            // Settings shows the allowance too.
            Task { await library.loadAccount(quietly: true) }
        case let .failure(error as AskError):
            if case .Stopped = error {
                turns[i].state = .stopped
                return
            }
            // The question goes back into the field, to send again once it's sorted.
            turns.remove(at: i)
            if draft.isEmpty { draft = question }
            switch error {
            case let .Upgrade(message, upgrade, planRequired):
                if !planRequired { questionsLeft = 0 }
                notice = AskNotice(message: message, pro: upgrade)
            case let .Wait(message, retryAfter):
                notice = AskNotice(message: retryAfter.map { "\(message) Try again in \(Self.wait($0))." } ?? message)
            case .Update:
                notice = AskNotice(message: "Update Linger to keep using Ask.")
            case .TooLarge:
                notice = AskNotice(message: "This conversation is too long to carry on. Close it and ask again.")
            case let .SignIn(message), let .Failed(message):
                notice = AskNotice(message: message)
            case .Stopped:
                break
            }
        case let .failure(error):
            turns.remove(at: i)
            if draft.isEmpty { draft = question }
            notice = AskNotice(message: message(of: error))
        }
    }

    /// "40 seconds", "5 minutes", "3 hours".
    private static func wait(_ seconds: UInt64) -> String {
        let (n, unit): (UInt64, String) = switch seconds {
        case 0..<90: (max(seconds, 1), "second")
        case 90..<5400: ((seconds + 59) / 60, "minute")
        default: ((seconds + 3599) / 3600, "hour")
        }
        return "\(n) \(unit)\(n == 1 ? "" : "s")"
    }

    /// A saved answer's verse citations, as its sources when it's reopened.
    private static func cited(_ text: String) -> [AskSource] {
        var out: [AskSource] = []
        for case let .verse(reference) in citationSegments(text: text) where !out.contains(.passage(reference: reference)) {
            out.append(.passage(reference: reference))
        }
        return out
    }
}

/// Core streams from the Ask thread; the model hears on the main actor.
private final class Relay: AskObserver, @unchecked Sendable {
    weak var model: AskModel?
    init(model: AskModel) { self.model = model }

    func event(event: AskEvent) {
        Task { @MainActor in self.model?.heard(event) }
    }
}

extension ItemKind {
    /// "sermon": what Ask calls it.
    var noun: String {
        switch self {
        case .sermon: "sermon"
        case .handwritten, .page: "photo"
        case .slide: "slide"
        case .note: "note"
        }
    }
}
