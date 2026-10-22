import LingerCore
import SwiftUI

/// The Bible tab: the chapter, with the jump field on top and quiet links to the chapters either side.
struct ReaderView: View {
    @Bindable var model: ReaderModel
    let library: Library
    @State private var selected: Set<UInt8> = []
    @State private var noteVerse: VerseRef?

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
                .task(id: "\(model.translation) \(model.book) \(model.chapter)") {
                    selected = []
                    await model.load()
                    scroll(proxy)
                }
                .onChange(of: model.scrollTarget) { if model.isShowingCurrent { scroll(proxy) } }
            }
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) { JumpField(model: model) }
                    .sharedBackgroundVisibility(.hidden)
            }
            .safeAreaInset(edge: .bottom) {
                if !selected.isEmpty { actions }
            }
            .toolbarVisibility(model.jumpOpen ? .hidden : .automatic, for: .tabBar)
            .sheet(item: $noteVerse) { verse in
                NoteSheet(library: library, verse: verse)
            }
        }
        .overlay {
            if model.jumpOpen { JumpPanel(model: model).transition(.identity) }
        }
    }

    /// Brings the target verse (or the chapter's top) into view once its chapter shows.
    private func scroll(_ proxy: ScrollViewProxy) {
        guard let target = model.scrollTarget else { return }
        model.scrollTarget = nil
        Task { proxy.scrollTo(target, anchor: target == 0 ? .top : UnitPoint(x: 0.5, y: 0.18)) }
    }

    private var selection: [VerseRef] {
        selected.sorted().map { VerseRef(book: model.book, chapter: model.chapter, verse: $0) }
    }

    /// What the selected verses can have: a colour, a bookmark, a note (one verse at a time).
    private var actions: some View {
        HStack(spacing: 14) {
            ForEach(highlightColors(), id: \.self) { color in
                Button { act { try $0.setHighlight(verses: selection, color: color) } } label: {
                    Circle().fill(color.swatch).frame(width: 28, height: 28)
                }
                .accessibilityLabel("Highlight \(String(describing: color))")
            }
            Button("Clear highlight", systemImage: "circle.slash") {
                act { try $0.setHighlight(verses: selection, color: nil) }
            }
            Button("Bookmark", systemImage: "bookmark") {
                act { study in try selection.forEach { _ = try study.toggleBookmark(verse: $0) } }
            }
            if selection.count == 1 {
                Button("Note", systemImage: "square.and.pencil") {
                    noteVerse = selection.first
                    selected = []
                }
            }
        }
        .labelStyle(.iconOnly)
        .font(.title3)
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
        .background(.regularMaterial, in: Capsule())
        .padding(.bottom, 8)
    }

    private func act(_ change: (Study) throws -> Void) {
        library.write(change)
        selected = []
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
                        VerseRow(verse: verse, marks: marks[verse.number], selected: selected.contains(verse.number),
                                 tinted: model.tinted == verse.number)
                            .id(verse.number)
                            .contentShape(Rectangle())
                            .onTapGesture {
                                model.tinted = nil
                                if selected.remove(verse.number) == nil { selected.insert(verse.number) }
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
