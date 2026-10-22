import LingerCore
import SwiftUI

struct ReaderView: View {
    @Bindable var model: ReaderModel
    let library: Library
    @State private var jump = ""
    @State private var selected: Set<UInt8> = []
    @State private var noteVerse: VerseRef?

    var body: some View {
        NavigationStack {
            ScrollView {
                chapter
                    .frame(maxWidth: 640, alignment: .leading)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 24)
                    .frame(maxWidth: .infinity)
            }
            .navigationTitle(model.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { toolbar }
            .searchable(text: $jump, prompt: "Go to a reference")
            .onSubmit(of: .search) {
                if model.go(to: jump) { jump = "" }
            }
            .task(id: "\(model.translation) \(model.book) \(model.chapter)") {
                selected = []
                await model.load()
            }
            .safeAreaInset(edge: .bottom) {
                if !selected.isEmpty { actions }
            }
            .sheet(item: $noteVerse) { verse in
                NoteSheet(library: library, verse: verse)
            }
        }
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
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(verses, id: \.number) { verse in
                        VerseRow(verse: verse, marks: marks[verse.number], selected: selected.contains(verse.number))
                            .onTapGesture {
                                if selected.remove(verse.number) == nil { selected.insert(verse.number) }
                            }
                    }
                    if let notice {
                        Text(notice).font(.footnote).foregroundStyle(.secondary).padding(.top, 16)
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

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            Button("Previous chapter", systemImage: "chevron.left", action: model.previousChapter)
                .disabled(!model.canGoBack)
        }
        ToolbarItemGroup(placement: .topBarTrailing) {
            Picker("Translation", selection: $model.translation) {
                ForEach(model.translations, id: \.id) { t in
                    Text(t.abbrev).tag(t.id)
                }
            }
            Button("Next chapter", systemImage: "chevron.right", action: model.nextChapter)
                .disabled(!model.canGoForward)
        }
    }
}

private struct VerseRow: View {
    let verse: Verse
    let marks: VerseMarks?
    let selected: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(verse.headings, id: \.self) { heading in
                Text(heading).font(.headline).padding(.top, 12)
            }
            (Text(number).font(.caption2).foregroundStyle(.secondary).baselineOffset(6)
                + Text(" " + verse.text)
                + margin)
                .font(.system(.body, design: .serif))
                .lineSpacing(5)
                .padding(.horizontal, 4)
                .background(marks?.highlight?.background ?? .clear, in: RoundedRectangle(cornerRadius: 4))
                .overlay {
                    if selected { RoundedRectangle(cornerRadius: 4).stroke(.tint, lineWidth: 2) }
                }
        }
        .contentShape(Rectangle())
    }

    /// The desktop's margin marks: a note, a bookmark.
    private var margin: Text {
        var marks = Text("")
        if self.marks?.note == true { marks = marks + Text(" ") + Text(Image(systemName: "note.text")) }
        if self.marks?.bookmark == true { marks = marks + Text(" ") + Text(Image(systemName: "bookmark.fill")) }
        return marks.font(.caption).foregroundStyle(.secondary)
    }

    private var number: String {
        verse.numberEnd > verse.number ? "\(verse.number)–\(verse.numberEnd)" : "\(verse.number)"
    }
}
