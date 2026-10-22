import LingerCore
import SwiftUI

/// The glass row that takes the tab bar's place while verses are selected: five highlight colours,
/// ··· and Ask, separated by space alone. Each control is a 44 pt target; the dots are drawn at
/// 20 pt inside theirs, about 24 pt apart.
struct VerseActionRow: View {
    let model: ReaderModel
    let library: Library
    let ask: () -> Void
    @State private var comparing = false

    var body: some View {
        let marks = library.marks(book: model.book, chapter: model.chapter)
        let selected = model.selection.map { Array($0) } ?? []
        HStack(spacing: 0) {
            ForEach(HighlightColor.row, id: \.self) { color in
                let on = !selected.isEmpty && selected.allSatisfy { marks[$0]?.highlight == color }
                Button {
                    let verses = model.selectedVerses
                    library.write { try $0.setHighlight(verses: verses, color: on ? nil : color) }
                } label: {
                    Circle().fill(color.swatch)
                        .frame(width: 20, height: 20)
                        .overlay { if on { Circle().strokeBorder(Theme.ink.opacity(0.55), lineWidth: 1.2).padding(-3) } }
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(on ? "Remove \(String(describing: color)) highlight" : "Highlight \(String(describing: color))")
                .accessibilityIdentifier("row-dot-\(color)")
            }
            Color.clear.frame(width: 10, height: 1)
            more(bookmarked: !selected.isEmpty && selected.allSatisfy { marks[$0]?.bookmark == true }, marks: marks)
            Color.clear.frame(width: 10, height: 1)
            Button(action: ask) {
                HStack(spacing: 5) {
                    Image(systemName: "bubble.left").font(.system(size: 14))
                    Text("Ask").font(Fonts.ui(15, weight: .medium))
                }
                .foregroundStyle(Theme.accent)
                .padding(.horizontal, 8)
                .frame(minWidth: 44, minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("row-ask")
        }
        .padding(.horizontal, 2)
        .glassEffect(Theme.glass, in: .capsule)
        .sheet(isPresented: $comparing) { CompareSheet(model: model) }
    }

    /// Study this verse (once the verse page exists), Compare, Bookmark and Share, in that order
    /// even though the menu opens upward. Copy is in the share sheet.
    private func more(bookmarked: Bool, marks: [UInt8: VerseMarks]) -> some View {
        Menu {
            Button("Study this verse", systemImage: "book") {}
                .disabled(true)
            Divider()
            Button("Compare", systemImage: "rectangle.split.2x1") { comparing = true }
            Button(bookmarked ? "Remove bookmark" : "Bookmark", systemImage: bookmarked ? "bookmark.slash" : "bookmark") {
                // Bookmark the verses that aren't yet, or take them all off.
                let verses = model.selectedVerses.filter { bookmarked || marks[$0.verse]?.bookmark != true }
                library.write { study in try verses.forEach { _ = try study.toggleBookmark(verse: $0) } }
            }
            ShareLink(item: "“\(model.selectedText)” \(model.selectionTitle ?? "") \(model.translationAbbrev)") {
                Label("Share", systemImage: "square.and.arrow.up")
            }
        } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: 17))
                .foregroundStyle(Theme.ink)
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .menuOrder(.fixed)
        .accessibilityLabel("More")
        .accessibilityIdentifier("row-more")
    }
}

/// The selected verses in each translation on the device.
private struct CompareSheet: View {
    let model: ReaderModel
    @State private var rows: [(TranslationInfo, String)]?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(rows ?? [], id: \.0.id) { t, text in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(t.abbrev).font(Fonts.ui(12, weight: .medium)).foregroundStyle(Theme.ink2)
                            Text(text).font(Fonts.serif(17)).foregroundStyle(Theme.ink)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.vertical, 12)
                        .overlay(alignment: .bottom) { Hairline() }
                    }
                }
                .padding(.horizontal, 22)
            }
            .overlay { if rows == nil { ProgressView() } }
            .background(Theme.page)
            .navigationTitle(model.selectionTitle ?? "")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { Button("Done") { dismiss() } }
        }
        .task { rows = await model.compare() }
    }
}

/// The toolbar +'s small glass menu: what can be added, and the verse it anchors to. Photo, Record
/// and Import join it with M4, M5 and imports.
struct CaptureMenu: View {
    let model: ReaderModel
    @Binding var anchor: UInt8
    let note: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 0) {
                Button(action: note) {
                    VStack(spacing: 6) {
                        Image(systemName: "square.and.pencil").font(.system(size: 19)).frame(height: 24)
                        Text("Note").font(Fonts.ui(12))
                    }
                    .foregroundStyle(Theme.ink)
                    .frame(width: 72, height: 52)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("capture-note")
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 6)
            .padding(.top, 12)
            .padding(.bottom, 10)
            Hairline().padding(.horizontal, 14)
            HStack(spacing: 6) {
                Image(systemName: "pin").font(.system(size: 11)).foregroundStyle(Theme.ink2)
                Text("Anchored to \(VerseRef(book: model.book, chapter: model.chapter, verse: anchor).display)")
                    .font(Fonts.ui(12))
                    .foregroundStyle(Theme.ink2)
                    .accessibilityIdentifier("capture-anchor")
                Spacer(minLength: 4)
                Menu {
                    Picker("Verse", selection: $anchor) {
                        ForEach(verseNumbers, id: \.self) { Text("\(model.title):\($0)").tag($0) }
                    }
                } label: {
                    Text("Change").font(Fonts.ui(12, weight: .medium)).foregroundStyle(Theme.accent)
                }
            }
            .padding(.horizontal, 16)
            .frame(height: 38)
        }
        .frame(width: 260)
        .glassEffect(Theme.glass, in: RoundedRectangle(cornerRadius: 24, style: .continuous))
    }

    private var verseNumbers: [UInt8] {
        guard case let .ready(verses, _) = model.content else { return [anchor] }
        return verses.map(\.number)
    }
}
