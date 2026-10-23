import LingerCore
import SwiftUI

/// The Bible tab: the chapter, with the jump field on top and quiet links to the chapters either side.
struct ReaderView: View {
    @Bindable var model: ReaderModel
    let library: Library
    /// Opens Ask with the selection.
    let ask: () -> Void
    @State private var noteVerse: VerseRef?
    @State private var addOpen = false
    /// The verse the + menu adds to.
    @State private var anchor: UInt8 = 1
    /// The verses whose page is open.
    @State private var studying: ClosedRange<UInt8>?

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                ScrollView {
                    chapter
                        .frame(maxWidth: 640, alignment: .leading)
                        .padding(.horizontal, 26)
                        .padding(.top, 4)
                        .padding(.bottom, 24)
                        .frame(maxWidth: .infinity)
                }
                .background(Theme.page)
                .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.readerFrame = $0 }
                .task(id: "\(model.translation) \(model.book) \(model.chapter)") {
                    await model.load()
                    scroll(proxy)
                }
                .onChange(of: model.scrollTarget) { if model.isShowingCurrent { scroll(proxy) } }
                .onChange(of: model.selection) { old, new in
                    addOpen = false
                    // Lift a newly tapped verse when the row would cover it.
                    guard old == nil, let n = new?.lowerBound,
                          (model.verseFrames[n]?.maxY ?? 0) > model.readerFrame.maxY - 110 else { return }
                    withAnimation(.smooth(duration: 0.35)) { proxy.scrollTo(n, anchor: UnitPoint(x: 0.5, y: 0.35)) }
                }
            }
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) { JumpField(model: model) }
                    .sharedBackgroundVisibility(.hidden)
                ToolbarItem(placement: .topBarTrailing) { addButton }
            }
            // The row takes the tab bar's place while verses are selected.
            .safeAreaBar(edge: .bottom, spacing: 0) {
                if model.selection != nil {
                    VerseActionRow(model: model, library: library, ask: ask) {
                        studying = model.selection
                        model.selection = nil
                    }
                        .padding(.bottom, 2)
                        .transition(.opacity.combined(with: .move(edge: .bottom)))
                }
            }
            .toolbarVisibility(model.jumpOpen || model.selection != nil ? .hidden : .automatic, for: .tabBar)
            .navigationDestination(item: $studying) { verses in
                VersePage(model: model, library: library, verses: verses, add: openAdd,
                          editNote: { noteVerse = $0 }, ask: ask)
            }
        }
        .sheet(item: $noteVerse) { verse in
            NoteSheet(library: library, verse: verse, quote: quote(verse.verse)) {
                model.selection = nil
            }
        }
        .overlay(alignment: .topTrailing) {
            if addOpen {
                ZStack(alignment: .topTrailing) {
                    Color.black.opacity(0.001)
                        .ignoresSafeArea()
                        .onTapGesture { withAnimation(.smooth(duration: 0.3)) { addOpen = false } }
                        .accessibilityHidden(true)
                    CaptureMenu(model: model, anchor: $anchor) {
                        addOpen = false
                        noteVerse = VerseRef(book: model.book, chapter: model.chapter, verse: anchor)
                    }
                    .padding(.trailing, 14)
                    .padding(.top, 58)
                    .transition(.opacity.combined(with: .scale(scale: 0.9, anchor: .topTrailing)))
                }
            }
        }
        .overlay {
            if model.jumpOpen { JumpPanel(model: model).transition(.identity) }
        }
    }

    /// The round glass +. Its menu follows the selection (its first verse; a note is on one verse),
    /// or the verse a jump landed on, or the chapter's first verse.
    private var addButton: some View {
        Button("Add", systemImage: "plus") { openAdd(model.selection?.lowerBound ?? model.tinted ?? 1) }
        .labelStyle(.iconOnly)
        .tint(Theme.ink)
        .rotationEffect(.degrees(addOpen ? 45 : 0))
        .accessibilityIdentifier("plus")
    }

    private func openAdd(_ verse: UInt8) {
        anchor = verse
        withAnimation(.smooth(duration: 0.3)) { addOpen.toggle() }
    }

    private func quote(_ n: UInt8) -> String {
        guard case let .ready(verses, _) = model.content else { return "" }
        return verses.first { ($0.number...$0.numberEnd).contains(n) }?.text ?? ""
    }

    /// Brings the target verse (or the chapter's top) into view once its chapter shows.
    private func scroll(_ proxy: ScrollViewProxy) {
        guard let target = model.scrollTarget else { return }
        model.scrollTarget = nil
        Task { proxy.scrollTo(target, anchor: target == 0 ? .top : UnitPoint(x: 0.5, y: 0.18)) }
    }

    /// Swaps the chapter in one frame, without cross-fading two chapters' text.
    private func go(_ place: Reference) {
        var swap = Transaction()
        swap.disablesAnimations = true
        withTransaction(swap) { model.go(book: place.book, chapter: place.chapter) }
    }

    @ViewBuilder
    private var chapter: some View {
        if let error = model.error {
            ContentUnavailableView("Can't show this chapter", systemImage: "exclamationmark.triangle",
                                   description: Text(error))
        } else {
            switch model.content {
            case nil, .pending:
                ProgressView().frame(maxWidth: .infinity)
            case let .ready(verses, notice):
                let marks = library.marks(book: model.book, chapter: model.chapter)
                VStack(alignment: .leading, spacing: 14) {
                    // The chapter's top, where a jump lands, includes the link back.
                    VStack(alignment: .leading, spacing: 14) {
                        if let previous = model.previous {
                            ChapterLink(target: previous, forward: false) { go(previous) }
                        }
                        heading
                    }
                    .id(UInt8(0))
                    ForEach(verses, id: \.number) { verse in
                        VerseRow(verse: verse, marks: marks[verse.number],
                                 selected: model.selection?.overlaps(verse.number...verse.numberEnd) == true,
                                 tinted: model.tinted == verse.number)
                            .id(verse.number)
                            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: {
                                model.verseFrames[verse.number] = $0
                            }
                            .contentShape(Rectangle())
                            .onTapGesture {
                                withAnimation(.smooth(duration: 0.3)) { model.tap(verse: verse.number) }
                            }
                            .accessibilityIdentifier("verse-\(verse.number)")
                            .accessibilityAddTraits(.isButton)
                    }
                    if let notice {
                        Text(notice).font(Fonts.ui(13)).foregroundStyle(Theme.ink2).padding(.top, 16)
                    }
                    if let next = model.next {
                        ChapterLink(target: next, forward: true) { go(next) }.padding(.top, 18)
                    }
                }
            case let .failed(message):
                ContentUnavailableView("Can't load this chapter", systemImage: "wifi.exclamationmark",
                                       description: Text(message))
            case .noKey:
                ContentUnavailableView("This translation needs an API key", systemImage: "key")
            case .missing:
                ContentUnavailableView("Not in this translation", systemImage: "book.closed")
            }
        }
    }

    /// "ROMANS" over a large "8".
    private var heading: some View {
        VStack(spacing: 2) {
            Text(model.books[Int(model.book)].name.uppercased())
                .font(Fonts.ui(11, weight: .medium))
                .tracking(1.6)
                .foregroundStyle(Theme.ink2)
            Text("\(model.chapter)")
                .font(Fonts.serif(44, weight: .light))
                .foregroundStyle(Theme.ink)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 14)
        .padding(.bottom, 10)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(model.title)
    }
}

private struct VerseRow: View {
    let verse: Verse
    let marks: VerseMarks?
    let selected: Bool
    let tinted: Bool
    private let size: CGFloat = 18

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(verse.headings, id: \.self) { heading in
                Text(heading).font(Fonts.serif(17, weight: .semibold)).foregroundStyle(Theme.ink).padding(.top, 12)
            }
            Text("\(Text(text))\(margin)")
                .font(Fonts.serif(size))
                .lineSpacing(size * 0.08)
                .foregroundStyle(Theme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, tinted ? 8 : 0)
                .padding(.horizontal, 10)
                .background {
                    if tinted { RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.accentSoft) }
                }
                .padding(.horizontal, -10)
        }
    }

    /// The number in the accent, then the verse with its highlight, underlined while selected.
    private var text: AttributedString {
        var number = AttributedString(numberText + "\u{2009}\u{2009}")
        number.font = Fonts.ui(size * 0.58, weight: .semibold)
        number.foregroundColor = Theme.accent
        number.baselineOffset = size * 0.16
        var body = AttributedString(verse.text)
        if let highlight = marks?.highlight { body.backgroundColor = highlight.background }
        if selected { body.underlineStyle = Text.LineStyle(pattern: .solid, color: Theme.accent.opacity(0.75)) }
        return number + body
    }

    /// The desktop's margin marks: a note, a bookmark in the accent.
    private var margin: Text {
        let note = marks?.note == true ? Text(" \(Image(systemName: "note.text"))").foregroundStyle(Theme.ink3) : Text("")
        let bookmark = marks?.bookmark == true ? Text(" \(Image(systemName: "bookmark.fill"))").foregroundStyle(Theme.accent) : Text("")
        return Text("\(note)\(bookmark)").font(.system(size: size * 0.6))
    }

    private var numberText: String {
        verse.numberEnd > verse.number ? "\(verse.number)–\(verse.numberEnd)" : "\(verse.number)"
    }
}
