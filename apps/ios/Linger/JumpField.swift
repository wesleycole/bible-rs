import LingerCore
import SwiftUI

/// The Bible tab's place field, "Romans 8 · KJV". Tapping it opens the navigator.
struct JumpField: View {
    let model: ReaderModel

    var body: some View {
        Button(action: model.openJump) {
            HStack(spacing: 7) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(Theme.ink2)
                Text("\(model.title) · \(model.translationAbbrev)")
                    .font(Fonts.ui(15))
                    .foregroundStyle(Theme.ink)
                    .lineLimit(1)
            }
            .padding(.horizontal, 16)
            .frame(minWidth: 214, alignment: .leading)
            .frame(height: 44)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .glassEffect(Theme.glassInteractive, in: .capsule)
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { model.jumpFieldFrame = $0 }
        // The open field takes over from here and grows from this frame.
        .opacity(model.jumpOpen ? 0 : 1)
        .accessibilityLabel("\(model.title) · \(model.translationAbbrev)")
        .accessibilityIdentifier("jump-field")
    }
}

/// The open field and the list under it, one tool: Recent places and the books with their chapter
/// grids while empty; a reference, a book or verse matches as you type.
struct JumpPanel: View {
    @Bindable var model: ReaderModel
    @State private var query = ""
    @State private var newTestament = true
    /// The book whose chapter grid is open; one at a time.
    @State private var expanded: UInt8?
    /// Verse matches, and the query and translation they answer.
    @State private var hits: [SearchHit] = []
    @State private var searched = ""
    @FocusState private var focused: Bool

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.bottom, 8)
                .background(Theme.page.ignoresSafeArea(edges: .top))
            // An opaque sheet that slides down from under the field, clipped at the field's bottom
            // edge, so the list never cross-fades over the text.
            ZStack(alignment: .top) {
                ScrollViewReader { proxy in
                    ScrollView {
                        VStack(alignment: .leading, spacing: 0) { results }
                            .padding(.horizontal, 22)
                            .padding(.bottom, 30)
                    }
                    .scrollDismissesKeyboard(.immediately)
                    .onAppear { startOnCurrentBook(proxy) }
                }
                .background(Theme.page)
                .offset(y: model.jumpExpanded ? 0 : -900)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .clipped()
        }
        // Run the list under the keyboard too, so the reader never shows behind it.
        .ignoresSafeArea(edges: .bottom)
        .onAppear {
            focused = true
            // Let the list settle on the current book first, so nothing scrolls during the slide.
            Task {
                try? await Task.sleep(for: .milliseconds(50))
                withAnimation(.easeOut(duration: 0.2)) { model.jumpExpanded = true }
            }
        }
        .onChange(of: query) { expanded = trimmed.isEmpty ? model.book : namedBooks.first?.index }
        .task(id: "\(model.translation)\n\(trimmed)") { await searchWords() }
        .onChange(of: model.translation) { focused = true }
    }

    // MARK: The field

    /// The field grows in place from the bar's capsule to full width; × takes the +'s place.
    private var header: some View {
        GeometryReader { geo in
            let box = geo.frame(in: .global)
            let from = model.jumpFieldFrame
            let grown = model.jumpExpanded || from == .zero
            ZStack(alignment: .topLeading) {
                field
                    .frame(width: grown ? box.width - 32 - 56 : from.width, height: 44)
                    .offset(x: grown ? 16 : from.minX - box.minX, y: grown ? 0 : from.midY - box.minY - 22)
                closeButton
                    .offset(x: box.width - 16 - 44)
                    .opacity(grown ? 1 : 0)
            }
        }
        .frame(height: 44)
        .padding(.top, 2)
    }

    private var field: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(Theme.ink2)
            // While it grows or shrinks, the field reads like the bar, not a placeholder.
            TextField(model.jumpExpanded ? "Book, reference or words" : "\(model.title) · \(model.translationAbbrev)",
                      text: $query)
                .font(Fonts.ui(16))
                .foregroundStyle(Theme.ink)
                .focused($focused)
                .submitLabel(.go)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .onSubmit(goToTop)
                .accessibilityIdentifier("jump-input")
            if !query.isEmpty {
                Button("Clear", systemImage: "xmark.circle.fill") { query = "" }
                    .labelStyle(.iconOnly)
                    .foregroundStyle(Theme.ink3)
                    .buttonStyle(.plain)
            }
            translationMenu
        }
        .padding(.leading, 16)
        .padding(.trailing, 6)
        .glassEffect(Theme.glassInteractive, in: .capsule)
    }

    private var closeButton: some View {
        Button(action: close) {
            Image(systemName: "xmark")
                .font(.system(size: 16, weight: .medium))
                .foregroundStyle(Theme.ink)
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .glassEffect(Theme.glassInteractive, in: .circle)
        .accessibilityLabel("Close")
        .accessibilityIdentifier("jump-close")
    }

    /// "KJV ⌄": changes the translation without closing the field.
    private var translationMenu: some View {
        Menu {
            Picker("Translation", selection: $model.translation) {
                ForEach(model.translations, id: \.id) { t in
                    VStack { Text(t.abbrev); Text(t.name) }.tag(t.id)
                }
            }
        } label: {
            HStack(spacing: 3) {
                Text(model.translationAbbrev).font(Fonts.ui(13, weight: .medium))
                Image(systemName: "chevron.down").font(.system(size: 9, weight: .semibold))
            }
            .foregroundStyle(Theme.ink)
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background(Theme.fill, in: Capsule())
            .contentShape(Capsule())
        }
        .menuOrder(.fixed)
        .accessibilityLabel("Translation, \(model.translationAbbrev)")
        .accessibilityIdentifier("jump-translation")
    }

    // MARK: What was typed

    private var trimmed: String { query.trimmingCharacters(in: .whitespaces) }

    /// "jn 3", "rom 8:28": a book with numbers after it. A leading ordinal ("1 co") isn't a chapter.
    private var reference: Reference? {
        let q = trimmed
        guard let letter = q.firstIndex(where: \.isLetter), q[letter...].contains(where: \.isNumber) else { return nil }
        return parseReference(input: q)
    }

    /// The books core reads in the typed letters, best first, typos included.
    private var suggestedBooks: [Book] {
        guard !trimmed.isEmpty, reference == nil else { return [] }
        var seen = Set<UInt8>()
        return suggestReferences(input: trimmed, limit: 66)
            .filter { seen.insert($0.book).inserted }
            .map { model.books[Int($0.book)] }
    }

    /// The suggested books when the letters look like a book's name ("jn", "rom", "1 co") rather
    /// than a word ("love"), which core also reads as a misspelt book. Words get verse matches,
    /// and a misspelt book ("revalation") its books only when no verse matches.
    private var namedBooks: [Book] {
        let books = suggestedBooks
        guard let first = books.first else { return [] }
        let q = trimmed
        let key = q.lowercased().filter(\.isLetter)
        let name = first.name.lowercased().filter(\.isLetter)
        return key.count <= 2 || name.hasPrefix(key) || q.first?.isNumber == true ? books : []
    }

    private func searchWords() async {
        let q = trimmed
        guard q.count >= 3, reference == nil, namedBooks.isEmpty else { return }
        // Wait out fast typing.
        try? await Task.sleep(for: .milliseconds(150))
        guard !Task.isCancelled else { return }
        let found = await model.search(q)
        guard !Task.isCancelled else { return }
        hits = found
        searched = "\(model.translation)\n\(q)"
    }

    // MARK: Results

    @ViewBuilder
    private var results: some View {
        let q = trimmed
        if q.isEmpty {
            browse
        } else if let reference {
            row(id: "jump-go", action: { go(reference) }) {
                Image(systemName: "arrow.right.circle.fill")
                    .font(.system(size: 16))
                    .foregroundStyle(Theme.accent)
                    .frame(width: 20)
                Text("Go to \(reference.display)")
                    .font(Fonts.ui(16, weight: .medium))
                    .foregroundStyle(Theme.ink)
            }
            .padding(.top, 4)
        } else if !namedBooks.isEmpty {
            sectionHeader(namedBooks.count == 1 ? "Book" : "Books")
            ForEach(namedBooks, id: \.index) { bookRow($0) }
        } else if q.count >= 3 && searched == "\(model.translation)\n\(q)" {
            if !hits.isEmpty {
                sectionHeader("Bible · \(hits.count == 50 ? "50+" : "\(hits.count)") \(hits.count == 1 ? "verse" : "verses")")
                ForEach(Array(hits.enumerated()), id: \.offset) { i, hit in
                    verseRow(hit, q).accessibilityIdentifier("jump-verse-\(i)")
                }
            } else if !suggestedBooks.isEmpty {
                sectionHeader("Books")
                ForEach(suggestedBooks.prefix(3), id: \.index) { bookRow($0) }
            } else {
                Text("No matches in \(model.translationAbbrev)")
                    .font(Fonts.ui(13))
                    .foregroundStyle(Theme.ink3)
                    .padding(.top, 16)
            }
        }
    }

    /// Recent places, then the books of one testament.
    @ViewBuilder
    private var browse: some View {
        let recents = model.recents.filter { ($0.book, $0.chapter) != (model.book, model.chapter) }
        if !recents.isEmpty {
            sectionHeader("Recent")
            ForEach(recents, id: \.self) { place in
                row(id: "recent-\(place.book)-\(place.chapter)", action: { go(place) }) {
                    Image(systemName: "clock")
                        .font(.system(size: 13))
                        .foregroundStyle(Theme.ink2)
                        .frame(width: 20)
                    Text(place.display).font(Fonts.ui(16)).foregroundStyle(Theme.ink)
                }
            }
        }
        Picker("Testament", selection: $newTestament.animation(.smooth(duration: 0.25))) {
            Text("Old Testament").tag(false)
            Text("New Testament").tag(true)
        }
        .pickerStyle(.segmented)
        .padding(.top, 22)
        .padding(.bottom, 6)
        .accessibilityIdentifier("testament")
        ForEach(model.books.filter { ($0.testament == .new) == newTestament }, id: \.index) { bookRow($0) }
    }

    /// A book as a plain row with a grey chapter count, which expands in place into its chapters.
    @ViewBuilder
    private func bookRow(_ book: Book) -> some View {
        let open = expanded == book.index
        Button {
            withAnimation(.smooth(duration: 0.25)) { expanded = open ? nil : book.index }
        } label: {
            HStack(spacing: 10) {
                Text(book.name)
                    .font(Fonts.ui(16, weight: open ? .medium : .regular))
                    .foregroundStyle(Theme.ink)
                Spacer(minLength: 0)
                Text("\(book.chapters)").font(Fonts.ui(13)).foregroundStyle(Theme.ink3)
                Image(systemName: "chevron.down")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(Theme.ink3)
                    .rotationEffect(.degrees(open ? 0 : -90))
            }
            .frame(minHeight: 46)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .overlay(alignment: .bottom) { if !open { Hairline() } }
        .id("book-\(book.index)")
        .accessibilityIdentifier("book-\(book.name)")
        if open { chapterGrid(book) }
    }

    /// Six columns of 44 pt chapters, the current one outlined.
    private func chapterGrid(_ book: Book) -> some View {
        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 6), count: 6), spacing: 6) {
            ForEach(1...Int(book.chapters), id: \.self) { n in
                let current = model.book == book.index && Int(model.chapter) == n
                Button {
                    go(Reference(book: book.index, chapter: UInt8(n), verse: nil, verseEnd: nil))
                } label: {
                    Text("\(n)")
                        .font(Fonts.ui(16, weight: current ? .semibold : .regular))
                        .foregroundStyle(current ? Theme.accent : Theme.ink)
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .background {
                            RoundedRectangle(cornerRadius: 10, style: .continuous)
                                .fill(current ? Theme.accentSoft : Theme.fill)
                        }
                        .overlay {
                            if current {
                                RoundedRectangle(cornerRadius: 10, style: .continuous)
                                    .strokeBorder(Theme.accent.opacity(0.5), lineWidth: 1)
                            }
                        }
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(book.name) \(n)")
                .accessibilityIdentifier("ch-\(book.name)-\(n)")
            }
        }
        .padding(.top, 2)
        .padding(.bottom, 12)
        .overlay(alignment: .bottom) { Hairline() }
        .transition(.opacity)
    }

    private func verseRow(_ hit: SearchHit, _ query: String) -> some View {
        Button { go(hit.reference) } label: {
            VStack(alignment: .leading, spacing: 3) {
                Text(hit.reference.display)
                    .font(Fonts.ui(12, weight: .medium))
                    .foregroundStyle(Theme.ink2)
                Text(bolded(hit.text, query))
                    .font(Fonts.serif(16))
                    .foregroundStyle(Theme.ink)
                    .lineLimit(3)
                    .multilineTextAlignment(.leading)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .overlay(alignment: .bottom) { Hairline() }
    }

    /// The verse with each typed word in bold where a word starts with it, as search matches.
    private func bolded(_ text: String, _ query: String) -> AttributedString {
        var out = AttributedString(text)
        let words = query.split(separator: " ").map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "\"-")) }
        for word in words where !word.isEmpty {
            var from = out.startIndex
            while let r = out[from...].range(of: word, options: [.caseInsensitive, .diacriticInsensitive]) {
                let atWordStart = r.lowerBound == out.startIndex
                    || !(out.characters[out.characters.index(before: r.lowerBound)].isLetter)
                if atWordStart { out[r].font = Fonts.serif(16, weight: .semibold) }
                from = r.upperBound
            }
        }
        return out
    }

    private func sectionHeader(_ title: String) -> some View {
        Text(title.uppercased())
            .font(Fonts.ui(11, weight: .medium))
            .tracking(1.2)
            .foregroundStyle(Theme.ink2)
            .padding(.top, 18)
            .padding(.bottom, 4)
    }

    private func row<Content: View>(id: String, action: @escaping () -> Void, @ViewBuilder content: () -> Content) -> some View {
        Button(action: action) {
            HStack(spacing: 12) {
                content()
                Spacer(minLength: 0)
            }
            .frame(minHeight: 46)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .overlay(alignment: .bottom) { Hairline() }
        .accessibilityIdentifier(id)
    }

    // MARK: Actions

    /// Opens on the current book's testament with its grid open, the book before it on top, so
    /// the next chapter is two taps away.
    private func startOnCurrentBook(_ proxy: ScrollViewProxy) {
        newTestament = model.books[Int(model.book)].testament == .new
        expanded = model.book
        let books = model.books.filter { ($0.testament == .new) == newTestament }
        guard let i = books.firstIndex(where: { $0.index == model.book }) else { return }
        let top = books[max(i - 1, 0)].index
        Task { proxy.scrollTo("book-\(top)", anchor: .top) }
    }

    private func close() {
        focused = false
        model.closeJump()
    }

    /// Swaps the chapter in one frame, without cross-fading two chapters' text.
    private func go(_ place: Reference) {
        focused = false
        model.closeJump()
        var swap = Transaction()
        swap.disablesAnimations = true
        withTransaction(swap) { model.go(book: place.book, chapter: place.chapter, verse: place.verse) }
    }

    /// Return takes the top result: the reference, the book's chapters, or the first verse.
    private func goToTop() {
        if let reference {
            go(reference)
        } else if let book = namedBooks.first {
            withAnimation(.smooth(duration: 0.25)) { expanded = book.index }
            focused = false
        } else if let hit = hits.first, searched == "\(model.translation)\n\(trimmed)" {
            go(hit.reference)
        }
    }
}

struct Hairline: View {
    var body: some View {
        Rectangle().fill(Theme.separator).frame(height: 1 / 3)
    }
}
