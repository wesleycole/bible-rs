import LingerCore
import SwiftUI

/// The Bible tab: the chapter, with the jump field on top; swiping sideways turns the chapter.
struct ReaderView: View {
    @Bindable var model: ReaderModel
    let library: Library
    /// Opens Ask about the selection, a page's verses or a library item.
    let ask: (AskAbout) -> Void
    /// Opens Library on the chapter's items.
    let openLibrary: (Reference) -> Void
    /// Opens a library item's page, from the field's "In your library".
    let openItem: (String) -> Void
    @State private var noteVerse: VerseRef?
    @State private var addOpen = false
    /// The verse the + menu adds to.
    @State private var anchor: UInt8 = 1
    /// The verses whose page is open.
    @State private var studying: ClosedRange<UInt8>?
    /// The verse whose margin popover is open.
    @State private var margin: UInt8?
    /// How far the text is swiped sideways: negative to the left, toward the next chapter;
    /// positive to the right, toward the previous one.
    @State private var swipe: CGFloat = 0
    /// Letting go this far over, or flicking, turns the chapter.
    private let turnDistance: CGFloat = 80

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
                        .offset(x: swipe)
                }
                .background(Theme.page)
                // Where the swipe goes, in the gap the text leaves.
                .overlay(alignment: swipe < 0 ? .trailing : .leading) {
                    if let target = swipeTarget {
                        HStack(spacing: 4) {
                            if swipe > 0 { Image(systemName: "chevron.left").font(.system(size: 11, weight: .semibold)) }
                            Text(target.display).font(Fonts.ui(14, weight: .medium))
                            if swipe < 0 { Image(systemName: "chevron.right").font(.system(size: 11, weight: .semibold)) }
                        }
                        .foregroundStyle(abs(swipe) >= turnDistance ? Theme.accent : Theme.ink2)
                        .fixedSize()
                        .frame(width: max(abs(swipe), 0))
                        .opacity(min(abs(swipe) / turnDistance, 1))
                        .accessibilityHidden(true)
                    }
                }
                .gesture(ChapterSwipe(changed: swiped, ended: endSwipe))
                .sensoryFeedback(.impact(weight: .light), trigger: swipeTarget != nil && abs(swipe) >= turnDistance) { _, ready in ready }
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
                    VerseActionRow(model: model, library: library, ask: { ask(model.askAbout()) }) {
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
                          editNote: { noteVerse = $0 }, ask: { ask(model.askAbout(verses)) })
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
            if model.jumpOpen { JumpPanel(model: model, library: library, openItem: openItem).transition(.identity) }
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

    /// The quiet library glyph beside a verse, which opens its popover.
    private func marginGlyph(_ n: UInt8, _ items: [ChapterItem]) -> some View {
        Button { margin = n } label: {
            Image(systemName: "books.vertical")
                .font(.system(size: 10))
                .foregroundStyle(Theme.ink3)
                .frame(width: 26, height: 26)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .popover(isPresented: Binding(get: { margin == n }, set: { if !$0 { margin = nil } }), arrowEdge: .leading) {
            MarginPopover(verse: VerseRef(book: model.book, chapter: model.chapter, verse: n),
                          items: Self.unique(items.filter { ($0.first...$0.last).contains(n) }.map(\.item)),
                          open: { margin = nil; studying = n...n },
                          ask: { item in
                              margin = nil
                              ask(model.askAbout(item: item))
                          })
                .presentationCompactAdaptation(.popover)
        }
        .accessibilityLabel("In your library")
        .accessibilityIdentifier("margin-\(n)")
    }

    /// Items once each: an item anchored twice in a chapter has two spans.
    static func unique(_ items: [LingerCore.LibraryItem]) -> [LingerCore.LibraryItem] {
        var seen = Set<String>()
        return items.filter { seen.insert($0.uid).inserted }
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

    /// The chapter the text is swiped toward, if any.
    private var swipeTarget: Reference? {
        swipe < 0 ? model.next : swipe > 0 ? model.previous : nil
    }

    /// The text follows the finger, stiffly where there's no chapter that way.
    private func swiped(_ x: CGFloat) {
        swipe = (x < 0 ? model.next : model.previous) == nil ? x / 4 : x
    }

    /// Far enough over, or a flick that way, slides the chapter out and its neighbour in from the
    /// other side; anything else springs back.
    private func endSwipe(_ x: CGFloat, velocity: CGFloat) {
        let forward = x < 0
        let width = max(model.readerFrame.width, 320)
        guard let target = forward ? model.next : model.previous,
              abs(x) >= turnDistance || (abs(velocity) > 500 && (velocity < 0) == forward && abs(x) > 16) else {
            withAnimation(.smooth(duration: 0.25)) { swipe = 0 }
            return
        }
        withAnimation(.easeIn(duration: 0.14)) { swipe = forward ? -width : width } completion: {
            var swap = Transaction()
            swap.disablesAnimations = true
            withTransaction(swap) {
                go(target)
                swipe = forward ? width : -width
            }
            // After the new chapter's first frame, so it slides from the far side.
            DispatchQueue.main.async { withAnimation(.smooth(duration: 0.24)) { swipe = 0 } }
        }
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
                let items = library.items(book: model.book, chapter: model.chapter)
                // One glyph where each item's verses start, without counts.
                let starts = Set(items.map(\.first))
                VStack(alignment: .leading, spacing: 14) {
                    heading(inLibrary: Self.unique(items.map(\.item)).count).id(UInt8(0))
                    ForEach(verses, id: \.number) { verse in
                        VerseRow(verse: verse, marks: marks[verse.number],
                                 selected: model.selection?.overlaps(verse.number...verse.numberEnd) == true,
                                 tinted: model.tinted == verse.number) {
                            if starts.contains(verse.number) { marginGlyph(verse.number, items) }
                        }
                            .id(verse.number)
                            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: {
                                model.verseFrames[verse.number] = $0
                            }
                            .onTapGesture {
                                withAnimation(.smooth(duration: 0.3)) { model.tap(verse: verse.number) }
                            }
                            // A container, so the margin glyph is its own control.
                            .accessibilityElement(children: .contain)
                            .accessibilityIdentifier("verse-\(verse.number)")
                    }
                    if let notice {
                        Text(notice).font(Fonts.ui(13)).foregroundStyle(Theme.ink2).padding(.top, 16)
                    }
                    if let next = model.next {
                        // Teaches the swipe; a tap works too.
                        Button { go(next) } label: {
                            Text("Swipe left for \(next.display)")
                                .font(Fonts.ui(13))
                                .foregroundStyle(Theme.ink3)
                                .frame(maxWidth: .infinity, minHeight: 44)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .padding(.top, 18)
                        .accessibilityLabel("Next chapter, \(next.display)")
                        .accessibilityIdentifier("next-chapter")
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

    /// "ROMANS" over a large "8", and a quiet "3 in your library ›" when the library has items on
    /// the chapter, which opens Library on them.
    private func heading(inLibrary count: Int) -> some View {
        VStack(spacing: 2) {
            VStack(spacing: 2) {
                Text(model.books[Int(model.book)].name.uppercased())
                    .font(Fonts.ui(11, weight: .medium))
                    .tracking(1.6)
                    .foregroundStyle(Theme.ink2)
                Text("\(model.chapter)")
                    .font(Fonts.serif(44, weight: .light))
                    .foregroundStyle(Theme.ink)
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel(model.title)
            .accessibilityAddTraits(.isHeader)
            // The swipe is a gesture VoiceOver can't make.
            .accessibilityActions {
                if let previous = model.previous { Button("Previous chapter, \(previous.display)") { go(previous) } }
                if let next = model.next { Button("Next chapter, \(next.display)") { go(next) } }
            }
            if count > 0 {
                Button { openLibrary(model.place) } label: {
                    HStack(spacing: 4) {
                        Text("\(count) in your library").font(Fonts.ui(13))
                        Image(systemName: "chevron.right").font(.system(size: 10, weight: .semibold))
                    }
                    .foregroundStyle(Theme.ink2)
                    .padding(.horizontal, 8)
                    .frame(minHeight: 32)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("chapter-library")
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 14)
        .padding(.bottom, 10)
    }
}

/// A sideways pan on the reader that leaves vertical drags to the scroll view: it only starts
/// when the finger moves mostly sideways.
private struct ChapterSwipe: UIGestureRecognizerRepresentable {
    let changed: (CGFloat) -> Void
    let ended: (CGFloat, _ velocity: CGFloat) -> Void

    func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
        let pan = UIPanGestureRecognizer()
        pan.delegate = context.coordinator
        return pan
    }

    func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

    func handleUIGestureRecognizerAction(_ pan: UIPanGestureRecognizer, context: Context) {
        let x = pan.translation(in: pan.view).x
        switch pan.state {
        case .changed: changed(x)
        case .ended: ended(x, pan.velocity(in: pan.view).x)
        case .cancelled, .failed: ended(0, 0)
        default: break
        }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        func gestureRecognizerShouldBegin(_ recognizer: UIGestureRecognizer) -> Bool {
            guard let pan = recognizer as? UIPanGestureRecognizer else { return false }
            let v = pan.velocity(in: pan.view)
            return abs(v.x) > abs(v.y) * 1.5
        }

        /// Alongside the scroll view's own pan, which this can't make wait.
        func gestureRecognizer(_ recognizer: UIGestureRecognizer,
                               shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool { true }
    }
}

private struct VerseRow<Margin: View>: View {
    let verse: Verse
    let marks: VerseMarks?
    let selected: Bool
    let tinted: Bool
    /// The library glyph, in the left margin by the verse's first line.
    @ViewBuilder let margin: Margin
    private let size: CGFloat = 18

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(verse.headings, id: \.self) { heading in
                Text(heading).font(Fonts.serif(17, weight: .semibold)).foregroundStyle(Theme.ink).padding(.top, 12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
            }
            Text(text)
                .accessibilityAddTraits(.isButton)
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
                // The whole line takes a tap. Set here, not on the row, so the margin glyph outside
                // the text's frame still takes its own.
                .contentShape(Rectangle())
                .overlay(alignment: .topLeading) {
                    margin.offset(x: -27, y: size * 0.7 - 13 + (tinted ? 8 : 0))
                }
                .overlay(alignment: .topTrailing) {
                    // A bookmark: a small glyph in the accent, in the right margin.
                    if marks?.bookmark == true {
                        Image(systemName: "bookmark.fill")
                            .font(.system(size: 10))
                            .foregroundStyle(Theme.accent.opacity(0.6))
                            .offset(x: 17, y: size * 0.35 + (tinted ? 8 : 0))
                            .accessibilityLabel("Bookmarked")
                    }
                }
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

    private var numberText: String {
        verse.numberEnd > verse.number ? "\(verse.number)–\(verse.numberEnd)" : "\(verse.number)"
    }
}
