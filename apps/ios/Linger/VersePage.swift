import LingerCore
import SwiftUI

/// The verse page (Study this verse): the verses in large serif, the colour dots, then what the
/// device knows about them. Your library, Reference and Cross references join it as the FFI
/// grows; until then those sections are absent, not empty. Its own Ask bar takes the tab bar's place.
struct VersePage: View {
    let model: ReaderModel
    let library: Library
    let verses: ClosedRange<UInt8>
    /// Opens the toolbar +'s menu anchored to a verse.
    let add: (UInt8) -> Void
    /// Edits a verse's note.
    let editNote: (VerseRef) -> Void
    let ask: () -> Void

    private var refs: [VerseRef] { verses.map { VerseRef(book: model.book, chapter: model.chapter, verse: $0) } }

    private var title: String {
        Reference(book: model.book, chapter: model.chapter, verse: verses.lowerBound,
                  verseEnd: verses.count > 1 ? verses.upperBound : nil).display
    }

    var body: some View {
        let marks = library.marks(book: model.book, chapter: model.chapter)
        let notes = refs.compactMap { ref -> (VerseRef, String)? in
            guard marks[ref.verse]?.note == true, let body = (try? library.study?.note(verse: ref))?.body, !body.isEmpty
            else { return nil }
            return (ref, body)
        }
        // The verse's own note shows as Your note, so it isn't listed again.
        let noteUids = Set(refs.compactMap { (try? library.study?.note(verse: $0))?.uid })
        let items = ReaderView.unique(library.items(book: model.book, chapter: model.chapter)
            .filter { verses.overlaps($0.first...$0.last) }.map(\.item))
            .filter { !noteUids.contains($0.uid) }
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Text(title.uppercased())
                    .font(Fonts.ui(11, weight: .medium))
                    .tracking(1.2)
                    .foregroundStyle(Theme.ink2)
                Text(text)
                    .font(Fonts.serif(24))
                    .lineSpacing(4)
                    .foregroundStyle(Theme.ink)
                    .padding(.top, 8)
                    .accessibilityIdentifier("page-text")
                dots(marks).padding(.top, 16)
                if !items.isEmpty {
                    section("Your library")
                    ForEach(items, id: \.uid) { ItemRow(item: $0).frame(minHeight: 46) }
                }
                if !notes.isEmpty {
                    section(notes.count == 1 ? "Your note" : "Your notes")
                    ForEach(notes, id: \.0.verse) { ref, body in
                        Button { editNote(ref) } label: {
                            VStack(alignment: .leading, spacing: 4) {
                                if verses.count > 1 {
                                    Text(ref.display).font(Fonts.ui(12, weight: .medium)).foregroundStyle(Theme.ink2)
                                }
                                Text(body).font(Fonts.serif(16)).foregroundStyle(Theme.ink).multilineTextAlignment(.leading)
                            }
                            .padding(.leading, 12)
                            .overlay(alignment: .leading) { Rectangle().fill(Theme.separator).frame(width: 2) }
                            .padding(.vertical, 6)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("page-note-\(ref.verse)")
                    }
                }
            }
            .frame(maxWidth: 640, alignment: .leading)
            .padding(.horizontal, 22)
            .padding(.top, 8)
            .padding(.bottom, 30)
            .frame(maxWidth: .infinity)
        }
        .background(Theme.page)
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarVisibility(.hidden, for: .tabBar)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Add", systemImage: "plus") { add(verses.lowerBound) }
                    .labelStyle(.iconOnly)
                    .tint(Theme.ink)
                    .accessibilityIdentifier("plus")
            }
        }
        .safeAreaBar(edge: .bottom, spacing: 0) { askBar }
    }

    private var text: String {
        guard case let .ready(all, _) = model.content else { return "" }
        return all.filter { verses.overlaps($0.number...$0.numberEnd) }.map(\.text).joined(separator: " ")
    }

    /// The five colours, for every verse on the page; the one they all have is ringed.
    private func dots(_ marks: [UInt8: VerseMarks]) -> some View {
        HStack(spacing: 14) {
            ForEach(HighlightColor.row, id: \.self) { color in
                let on = verses.allSatisfy { marks[$0]?.highlight == color }
                Button {
                    let refs = self.refs
                    library.write { try $0.setHighlight(verses: refs, color: on ? nil : color) }
                } label: {
                    Circle().fill(color.swatch).frame(width: 22, height: 22)
                        .overlay { if on { Circle().strokeBorder(Theme.ink.opacity(0.55), lineWidth: 1.2).padding(-3) } }
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(on ? "Remove \(String(describing: color)) highlight" : "Highlight \(String(describing: color))")
                .accessibilityIdentifier("page-dot-\(color)")
            }
        }
        .padding(.leading, -11)
    }

    private func section(_ title: String) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Hairline().padding(.bottom, 16)
            Text(title.uppercased())
                .font(Fonts.ui(11, weight: .medium))
                .tracking(0.8)
                .foregroundStyle(Theme.ink2)
                .padding(.bottom, 8)
        }
        .padding(.top, 24)
    }

    /// Looks like a composer; opens Ask about these verses.
    private var askBar: some View {
        Button(action: ask) {
            HStack(spacing: 10) {
                Image(systemName: "bubble.left").font(.system(size: 15)).foregroundStyle(Theme.accent)
                Text("Ask about this verse").font(Fonts.ui(16)).foregroundStyle(Theme.ink2)
                Spacer()
                Image(systemName: "arrow.up").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.onAccent)
                    .frame(width: 34, height: 34)
                    .background(Theme.ink3, in: Circle())
            }
            .padding(.leading, 18)
            .padding(.trailing, 7)
            .frame(height: 50)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .glassEffect(Theme.glassInteractive, in: .capsule)
        .padding(.horizontal, 14)
        .padding(.bottom, 6)
        .accessibilityIdentifier("page-ask")
    }
}
