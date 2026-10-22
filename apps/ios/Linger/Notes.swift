import LingerCore
import SwiftUI

/// A verse's note, saved as the sheet closes; an empty note is deleted.
struct NoteSheet: View {
    let library: Library
    let verse: VerseRef
    @State private var text = ""
    @FocusState private var editing: Bool
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            TextEditor(text: $text)
                .focused($editing)
                .font(.system(.body, design: .serif))
                .padding(.horizontal)
                .navigationTitle(verse.display)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    Button("Done") { dismiss() }
                }
        }
        .onAppear {
            text = (try? library.study?.note(verse: verse))?.body ?? ""
            editing = true
        }
        .onDisappear { library.write { try $0.setNote(verse: verse, body: text) } }
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
