import LingerCore
import SwiftUI

/// A verse's note. Save keeps it (an empty note is deleted); cancelling leaves it as it was, and
/// leaves the verse selection for the next action.
struct NoteSheet: View {
    let library: Library
    let verse: VerseRef
    /// The verse's text, quoted above the note.
    let quote: String
    let saved: () -> Void
    @State private var text = ""
    @State private var original = ""
    @FocusState private var editing: Bool
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 14) {
                HStack(alignment: .top, spacing: 12) {
                    Rectangle().fill(Theme.accent.opacity(0.6)).frame(width: 2)
                    Text(quote).font(Fonts.serif(15)).foregroundStyle(Theme.ink2).lineLimit(3)
                }
                .fixedSize(horizontal: false, vertical: true)
                Hairline()
                TextField("Write a note…", text: $text, axis: .vertical)
                    .focused($editing)
                    .font(Fonts.serif(18))
                    .foregroundStyle(Theme.ink)
                    .accessibilityIdentifier("note-field")
                Spacer()
            }
            .padding(.horizontal, 22)
            .padding(.top, 8)
            .background(Theme.page)
            .navigationTitle(verse.display)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                        .accessibilityIdentifier("note-cancel")
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") {
                        library.write { try $0.setNote(verse: verse, body: text) }
                        saved()
                        dismiss()
                    }
                    .accessibilityIdentifier("note-save")
                }
            }
        }
        // Large, so the keyboard can't push the bar under the Dynamic Island; a changed note
        // isn't lost to a swipe.
        .presentationDetents([.large])
        .presentationBackground(Theme.page)
        .interactiveDismissDisabled(text != original)
        .onAppear {
            original = (try? library.study?.note(verse: verse))?.body ?? ""
            text = original
            editing = true
        }
    }
}

/// Every verse note, in canonical order; choosing one opens its chapter. Library's page until the
/// library comes to the iPhone.
struct NotesList: View {
    let library: Library
    let open: (VerseRef) -> Void

    var body: some View {
        let _ = library.generation
        let notes = library.study?.notes() ?? []
        NavigationStack {
            List(notes, id: \.note.uid) { note in
                Button {
                    open(note.verse)
                } label: {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(note.reference).font(.headline)
                        Text(note.note.body).lineLimit(3).foregroundStyle(.secondary)
                    }
                }
                .foregroundStyle(.primary)
            }
            .overlay {
                if notes.isEmpty {
                    ContentUnavailableView("No notes yet", systemImage: "note.text",
                                           description: Text("Select a verse and tap the pencil to write one."))
                }
            }
            .navigationTitle("Library")
        }
    }
}

extension VerseRef: @retroactive Identifiable {
    public var id: Self { self }

    /// "John 3:16".
    var display: String {
        Reference(book: book, chapter: chapter, verse: verse, verseEnd: nil).display
    }
}
