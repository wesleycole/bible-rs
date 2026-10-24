import LingerCore
import SwiftUI

/// Ask: a full-height sheet over the reader. Its context and scope at the top, suggested questions
/// until something is asked, then the chat; one composer at the foot whose button sends, or stops
/// while an answer is written.
struct AskSheet: View {
    @Bindable var ask: AskModel
    let library: Library
    let reader: ReaderModel
    /// Opens a cited verse in the Bible tab, or a library item on its page (at a moment).
    let openVerse: (Reference) -> Void
    let openItem: (String, UInt64?) -> Void
    @State private var history = false
    @State private var consenting = false
    @State private var showingPro = false
    /// The answer whose sources are unfolded, and the source whose peek card shows.
    @State private var unfolded: UUID?
    @State private var peek: AskSource?
    @FocusState private var composing: Bool

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        if ask.turns.isEmpty {
                            context.padding(.top, 8)
                            suggestions.padding(.top, 34)
                        }
                        ForEach(ask.turns) { turn in
                            TurnView(turn: turn, library: library, unfolded: $unfolded, peek: $peek,
                                     save: { ask.save(turn) })
                                .id(turn.id)
                        }
                        if let peek {
                            PeekCard(source: peek, library: library, reader: reader,
                                     open: { open(peek) }, close: { self.peek = nil })
                                .padding(.top, 14)
                                .id("peek")
                        }
                        if let notice = ask.notice {
                            noticeView(notice).padding(.top, 18)
                        }
                    }
                    .frame(maxWidth: 640, alignment: .leading)
                    .padding(.horizontal, 22)
                    .padding(.bottom, 24)
                    .frame(maxWidth: .infinity)
                }
                .scrollDismissesKeyboard(.interactively)
                .onChange(of: ask.turns.last?.text) { scrollToEnd(proxy) }
                .onChange(of: ask.turns.count) { scrollToEnd(proxy) }
                // The answer's actions and sources line appear once it's done.
                .onChange(of: ask.turns.last?.state) { scrollToEnd(proxy) }
                .onChange(of: peek) { if peek != nil { withAnimation { proxy.scrollTo("peek", anchor: .bottom) } } }
                // Unfolded sources come into view above the composer.
                .onChange(of: unfolded) { if let id = unfolded { withAnimation { proxy.scrollTo(id, anchor: .bottom) } } }
            }
            .background(Theme.page)
            .navigationTitle("Ask")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark", action: ask.close)
                        .tint(Theme.ink)
                        .accessibilityIdentifier("ask-close")
                }
                ToolbarItem(placement: .primaryAction) {
                    Button("History", systemImage: "clock") { history = true }
                        .tint(Theme.ink)
                        .accessibilityIdentifier("ask-history")
                }
            }
            .safeAreaBar(edge: .bottom, spacing: 0) { foot }
        }
        .presentationDetents([.large])
        .presentationBackground(Theme.page)
        // Questions left show under the composer before the first answer reports them.
        .task { if library.account == nil { await library.loadAccount(quietly: true) } }
        .sheet(isPresented: $history) {
            AskHistory(library: library) { id in
                history = false
                peek = nil
                ask.open(chat: id, about: about)
            }
        }
        .sheet(isPresented: $consenting) { AskConsentSheet { accepted in
            consenting = false
            if accepted {
                ask.consented = true
                ask.send()
            }
        } }
        .sheet(isPresented: $showingPro) { ProSheet(library: library) }
    }

    /// What's on screen now, for a question from history.
    private var about: AskAbout {
        AskAbout(book: reader.book, chapter: reader.chapter, translation: reader.translation,
                 translationAbbrev: reader.translationAbbrev)
    }

    // MARK: Context

    /// The quoted selection with × to drop it, "Reading Romans 8", or the item asked about; the
    /// scope under it.
    @ViewBuilder
    private var context: some View {
        if let about = ask.about {
            VStack(alignment: .leading, spacing: 10) {
                if let item = about.item {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(item.kind.origin).font(Fonts.ui(12)).foregroundStyle(Theme.ink2)
                        Text(item.title).font(Fonts.serif(22)).foregroundStyle(Theme.ink)
                    }
                } else if let title = about.selectionTitle {
                    HStack(alignment: .top, spacing: 14) {
                        VStack(alignment: .leading, spacing: 10) {
                            Text(about.quote).font(Fonts.serif(20)).lineSpacing(3).foregroundStyle(Theme.ink)
                            Text("\(title) · \(about.translationAbbrev)".uppercased())
                                .font(Fonts.ui(11, weight: .medium)).tracking(1.4).foregroundStyle(Theme.ink2)
                        }
                        Spacer(minLength: 0)
                        Button("Remove the selection", systemImage: "xmark") { ask.dropSelection() }
                            .labelStyle(.iconOnly)
                            .font(.system(size: 13, weight: .medium))
                            .foregroundStyle(Theme.ink2)
                            .frame(width: 44, height: 44)
                            .padding(.top, -10)
                            .padding(.trailing, -12)
                            .accessibilityIdentifier("ask-drop-selection")
                    }
                    .padding(.leading, 16)
                    .overlay(alignment: .leading) { Rectangle().fill(Theme.accent).frame(width: 2) }
                } else {
                    Text("Reading \(about.place.display)")
                        .font(Fonts.serif(20)).foregroundStyle(Theme.ink)
                }
                // Only where it's a real choice: the library is reachable, and the question isn't
                // about one of its items.
                if about.item == nil, library.librarySync != nil { scopeMenu.padding(.leading, about.selection == nil ? 0 : 16) }
            }
        }
    }

    private var scopeMenu: some View {
        Menu {
            Picker("Answer from", selection: $ask.scope) {
                Text("Bible + library").tag(AskScope.bibleAndLibrary)
                Text("Bible only").tag(AskScope.bibleOnly)
            }
        } label: {
            HStack(spacing: 4) {
                Text(ask.scope == .bibleOnly ? "Bible only" : "Bible + library").font(Fonts.ui(14))
                Image(systemName: "chevron.down").font(.system(size: 10, weight: .semibold))
            }
            .foregroundStyle(Theme.ink2)
            .frame(minHeight: 32)
            .contentShape(Rectangle())
        }
        .menuOrder(.fixed)
        .accessibilityIdentifier("ask-scope")
    }

    @ViewBuilder
    private var suggestions: some View {
        let questions = ask.suggestions
        if !questions.isEmpty {
            Text("Suggested").font(Fonts.ui(14)).foregroundStyle(Theme.ink2).padding(.bottom, 4)
            ForEach(Array(questions.enumerated()), id: \.offset) { i, question in
                Button { send(question) } label: {
                    HStack(spacing: 10) {
                        Text(question).font(Fonts.serif(18)).foregroundStyle(Theme.ink).multilineTextAlignment(.leading)
                        Spacer(minLength: 0)
                        Image(systemName: "chevron.right").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.ink3)
                    }
                    .frame(minHeight: 52)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .overlay(alignment: .bottom) { Hairline() }
                .disabled(ask.spent)
                .accessibilityIdentifier("ask-suggestion-\(i)")
            }
        }
    }

    private func noticeView(_ notice: AskNotice) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(notice.message).font(Fonts.ui(14)).foregroundStyle(Theme.ink2)
            if notice.pro {
                Button("See Pro") { showingPro = true }.font(Fonts.ui(14, weight: .medium)).foregroundStyle(Theme.accent)
            }
        }
        .accessibilityIdentifier("ask-notice")
    }

    // MARK: The composer

    private var foot: some View {
        VStack(spacing: 8) {
            if ask.spent {
                // Out of questions: plain text above a disabled composer, with a link to Pro.
                HStack(spacing: 4) {
                    Text("You've used this month's questions.").foregroundStyle(Theme.ink2)
                    Button("See Pro") { showingPro = true }.foregroundStyle(Theme.accent)
                }
                .font(Fonts.ui(13))
            }
            composer
            if let left = ask.questionsLeft ?? library.account?.questionsLeft, !ask.spent {
                Text("\(left) question\(left == 1 ? "" : "s") left this month")
                    .font(Fonts.ui(12)).foregroundStyle(Theme.ink3)
                    .accessibilityIdentifier("ask-left")
            }
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 4)
    }

    private var composer: some View {
        let ready = !ask.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !ask.spent
        return HStack(alignment: .bottom, spacing: 8) {
            TextField(ask.placeholder, text: $ask.draft, axis: .vertical)
                .lineLimit(1...5)
                .font(Fonts.ui(16))
                .foregroundStyle(Theme.ink)
                .focused($composing)
                .disabled(ask.answering || ask.spent)
                .padding(.vertical, 13)
                .onSubmit { send(nil) }
                .accessibilityIdentifier("ask-field")
            Button {
                if ask.answering { ask.stop() } else { send(nil) }
            } label: {
                Image(systemName: ask.answering ? "stop.fill" : "arrow.up")
                    .font(.system(size: ask.answering ? 13 : 15, weight: .semibold))
                    .foregroundStyle(Theme.onAccent)
                    .frame(width: 36, height: 36)
                    .background(ask.answering || ready ? Theme.accent : Theme.ink3.opacity(0.6), in: Circle())
                    .frame(width: 44, height: 44)
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .disabled(!ask.answering && !ready)
            .accessibilityLabel(ask.answering ? "Stop" : "Send")
            .accessibilityIdentifier("ask-send")
            .padding(.bottom, 2)
        }
        .padding(.leading, 20)
        .padding(.trailing, 4)
        .glassEffect(Theme.glassInteractive, in: RoundedRectangle(cornerRadius: 26, style: .continuous))
    }

    // MARK: Actions

    private func send(_ question: String?) {
        if let question { ask.draft = question }
        guard ask.consented else {
            consenting = true
            return
        }
        composing = false
        peek = nil
        ask.send()
    }

    /// Open in Bible, or Open: collapses the sheet and leaves the return bar.
    private func open(_ source: AskSource) {
        peek = nil
        ask.leave()
        switch source {
        case let .passage(reference): openVerse(reference)
        case let .item(uid, _, _, atMs, _): openItem(uid, atMs)
        }
    }

    private func scrollToEnd(_ proxy: ScrollViewProxy) {
        guard let last = ask.turns.last else { return }
        proxy.scrollTo(last.id, anchor: .bottom)
    }
}

/// A question in a grey bubble, the answer in serif with its citations in the accent, the
/// answer's own actions, and the line that folds its sources.
private struct TurnView: View {
    let turn: AskTurn
    let library: Library
    @Binding var unfolded: UUID?
    @Binding var peek: AskSource?
    let save: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Spacer(minLength: 60)
                Text(turn.question)
                    .font(Fonts.ui(16))
                    .foregroundStyle(Theme.ink)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 11)
                    .background(Theme.fill, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
            }
            .padding(.top, 18)
            if !turn.text.isEmpty {
                Text(answer)
                    .font(Fonts.serif(18))
                    .lineSpacing(4)
                    .foregroundStyle(Theme.ink)
                    .padding(.top, 22)
                    .environment(\.openURL, OpenURLAction { url in
                        guard let source = source(url) else { return .discarded }
                        peek = source
                        return .handled
                    })
                    .accessibilityIdentifier("ask-answer")
            }
            if turn.state == .done { actions.padding(.top, 14) }
            line.padding(.top, turn.state == .done ? 6 : 16)
            if unfolded == turn.id { sourceGroups }
        }
    }

    /// Save to library and Share, under the answer they belong to. Copy is in the share sheet.
    private var actions: some View {
        HStack(spacing: 6) {
            Button(action: save) {
                Image(systemName: turn.saved ? "bookmark.fill" : "bookmark")
                    .font(.system(size: 16))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .disabled(turn.saved)
            .accessibilityLabel(turn.saved ? "Saved to library" : "Save to library")
            .accessibilityIdentifier("ask-save")
            ShareLink(item: "\(turn.question)\n\n\(plain(turn.text))") {
                Image(systemName: "square.and.arrow.up")
                    .font(.system(size: 16))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Share")
            .accessibilityIdentifier("ask-share")
        }
        .buttonStyle(.plain)
        .foregroundStyle(Theme.ink2)
        .padding(.leading, -12)
    }

    /// "Reading 1 sermon and 1 note from your library…" with a spinner; once done, "Read … ›",
    /// which unfolds the sources.
    @ViewBuilder
    private var line: some View {
        if let text = turn.sourcesLine {
            if turn.state == .done && !turn.sources.isEmpty {
                Button {
                    withAnimation(.smooth(duration: 0.25)) { unfolded = unfolded == turn.id ? nil : turn.id }
                } label: {
                    HStack(spacing: 6) {
                        Text(text).font(Fonts.ui(14)).foregroundStyle(Theme.ink2).multilineTextAlignment(.leading)
                        Image(systemName: "chevron.right")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(Theme.ink3)
                            .rotationEffect(.degrees(unfolded == turn.id ? 90 : 0))
                    }
                    .frame(minHeight: 36)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("ask-sources")
            } else {
                HStack(spacing: 8) {
                    if turn.state == .answering { ProgressView().controlSize(.small) }
                    Text(text).font(Fonts.ui(14)).foregroundStyle(Theme.ink2)
                }
                .frame(minHeight: 36)
                .accessibilityIdentifier("ask-reading")
            }
        }
    }

    /// From the Bible, then From your library. From reference works joins with M7.
    private var sourceGroups: some View {
        let passages = turn.sources.filter { if case .passage = $0 { true } else { false } }
        let items = turn.sources.filter { if case .item = $0 { true } else { false } }
        return VStack(alignment: .leading, spacing: 0) {
            if !passages.isEmpty { group("From the Bible", id: "bible", passages) }
            if !items.isEmpty { group("From your library", id: "library", items) }
        }
        .padding(.top, 4)
    }

    private func group(_ title: String, id: String, _ sources: [AskSource]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title.uppercased()).font(Fonts.ui(11, weight: .medium)).tracking(1).foregroundStyle(Theme.ink2)
                .padding(.top, 12).padding(.bottom, 2)
            ForEach(Array(sources.enumerated()), id: \.offset) { i, source in
                Button { peek = source } label: {
                    HStack(spacing: 10) {
                        Image(systemName: source.symbol).font(.system(size: 13)).foregroundStyle(Theme.ink2).frame(width: 18)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(source.title).font(Fonts.ui(15)).foregroundStyle(Theme.ink).lineLimit(1)
                            if let detail = source.detail {
                                Text(detail).font(Fonts.ui(12)).foregroundStyle(Theme.ink2)
                            }
                        }
                        Spacer(minLength: 0)
                    }
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .overlay(alignment: .bottom) { Hairline() }
                .accessibilityIdentifier("ask-source-\(id)-\(i)")
            }
        }
    }

    /// The answer with each citation as a link in the accent: a verse by its reference, a library
    /// item by what it is ("Your sermon").
    private var answer: AttributedString {
        var out = AttributedString()
        for segment in citationSegments(text: turn.text) {
            switch segment {
            case let .text(text):
                // The model writes inline Markdown: *emphasis* and **strong**.
                out += (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
                    ?? AttributedString(text)
            case let .verse(r):
                var link = AttributedString(r.display)
                link.link = URL(string: "linger-verse:\(r.book):\(r.chapter):\(r.verse ?? 0):\(r.verseEnd ?? 0)")
                link.foregroundColor = Theme.accent
                out += link
            case let .item(uid, atMs, page):
                let item = try? library.study?.libraryItem(uid: uid)
                var link = AttributedString(item.map { $0.kind.origin } ?? "Your library")
                link.font = Fonts.ui(13)
                link.link = URL(string: "linger-item:\(uid):\(atMs ?? 0):\(page ?? 0)")
                link.foregroundColor = Theme.accent
                link.backgroundColor = Theme.accentSoft
                out += AttributedString(" ") + link
            case .sermon:
                out += AttributedString("(a sermon)")
            }
        }
        return out
    }

    private func source(_ url: URL) -> AskSource? {
        let parts = url.absoluteString.split(separator: ":").map(String.init)
        switch parts.first {
        case "linger-verse":
            guard parts.count == 5, let book = UInt8(parts[1]), let chapter = UInt8(parts[2]),
                  let verse = UInt8(parts[3]), let end = UInt8(parts[4]) else { return nil }
            return .passage(reference: Reference(book: book, chapter: chapter, verse: verse == 0 ? nil : verse,
                                                 verseEnd: end == 0 ? nil : end))
        case "linger-item":
            guard parts.count == 4, let item = try? library.study?.libraryItem(uid: parts[1]) else { return nil }
            let at = UInt64(parts[2]) ?? 0, page = UInt32(parts[3]) ?? 0
            return .item(uid: item.uid, kind: item.kind, title: item.title, atMs: at == 0 ? nil : at, page: page == 0 ? nil : page)
        default:
            return nil
        }
    }
}

/// A source's verse text or excerpt in the chat, with Open in Bible (or Open).
private struct PeekCard: View {
    let source: AskSource
    let library: Library
    let reader: ReaderModel
    let open: () -> Void
    let close: () -> Void
    @State private var text: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 2) {
                    if let detail = source.detail {
                        Text(detail).font(Fonts.ui(12)).foregroundStyle(Theme.ink2)
                    }
                    Text(source.title).font(Fonts.serif(18, weight: .medium)).foregroundStyle(Theme.ink)
                }
                Spacer(minLength: 0)
                Button("Close", systemImage: "xmark", action: close)
                    .labelStyle(.iconOnly)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(Theme.ink2)
                    .frame(width: 44, height: 44)
                    .padding(.trailing, -12)
                    .padding(.top, -12)
            }
            if let text {
                Text(text).font(Fonts.serif(16)).lineSpacing(3).foregroundStyle(Theme.ink).lineLimit(8)
            }
            Button(action: open) {
                Text(source.isPassage ? "Open in Bible" : "Open").font(Fonts.ui(15, weight: .medium))
                    .foregroundStyle(Theme.accent)
                    .frame(minHeight: 36)
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("peek-open")
        }
        .padding(16)
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay { RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(Theme.separator, lineWidth: 0.5) }
        .task(id: source) { text = await load() }
    }

    /// The verses in the reader's translation, or the item's excerpt.
    private func load() async -> String? {
        switch source {
        case let .passage(r):
            return await reader.text(of: r)
        case let .item(uid, _, _, atMs, _):
            guard let study = library.study, let item = try? study.libraryItem(uid: uid) else { return nil }
            if let atMs, case let .transcript(segments) = try? study.libraryItemText(uid: uid),
               let at = segments.last(where: { $0.startMs <= atMs }) ?? segments.first {
                return at.text
            }
            return item.excerpt ?? item.summary ?? {
                if case let .plain(text) = try? study.libraryItemText(uid: uid) { return text }
                return nil
            }()
        }
    }
}

/// The one-time sheet before the first question: what is sent where (core's words, as on the Mac).
private struct AskConsentSheet: View {
    let answer: (Bool) -> Void
    private let text = askConsentText()

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text(text.title).font(Fonts.serif(26)).foregroundStyle(Theme.ink)
            ForEach(text.points, id: \.self) { point in
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    Circle().fill(Theme.ink3).frame(width: 5, height: 5)
                    Text(point).font(Fonts.ui(15)).foregroundStyle(Theme.ink2)
                }
            }
            Spacer()
            Button { answer(true) } label: {
                Text("Continue").font(Fonts.ui(16, weight: .medium)).frame(maxWidth: .infinity, minHeight: 50)
            }
            .buttonStyle(.borderedProminent)
            .accessibilityIdentifier("ask-consent-continue")
            Button("Not Now") { answer(false) }.frame(maxWidth: .infinity)
        }
        .padding(28)
        .presentationDetents([.medium])
        .presentationBackground(Theme.page)
    }
}

/// An answer's text with its citations written out, to share.
func plain(_ text: String) -> String {
    citationSegments(text: text).map { segment in
        switch segment {
        case let .text(text): text
        case let .verse(r): r.display
        case .item, .sermon: ""
        }
    }.joined()
}

extension AskSource: @retroactive Identifiable {
    public var id: String {
        switch self {
        case let .passage(r): "p\(r.book):\(r.chapter):\(r.verse ?? 0):\(r.verseEnd ?? 0)"
        case let .item(uid, _, _, atMs, page): "i\(uid):\(atMs ?? 0):\(page ?? 0)"
        }
    }

    var isPassage: Bool { if case .passage = self { true } else { false } }

    var title: String {
        switch self {
        case let .passage(r): r.display
        case let .item(_, _, title, _, _): title
        }
    }

    /// "Your sermon · 12:30", or nothing for a passage.
    var detail: String? {
        guard case let .item(_, kind, _, atMs, page) = self else { return nil }
        if let atMs { return "\(kind.origin) · \(clock(atMs))" }
        if let page { return "\(kind.origin) · page \(page)" }
        return kind.origin
    }

    var symbol: String {
        switch self {
        case .passage: "book"
        case let .item(_, kind, _, _, _): kind.symbol
        }
    }
}

/// "12:30" or "1:02:05".
func clock(_ ms: UInt64) -> String {
    let s = ms / 1000
    return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
}
