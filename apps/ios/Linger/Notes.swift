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

/// Every verse note, in canonical order; choosing one opens its chapter.
struct NotesList: View {
    let library: Library
    let open: (VerseRef) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let _ = library.generation
        let notes = library.study?.notes() ?? []
        NavigationStack {
            List(notes, id: \.note.uid) { note in
                Button {
                    open(note.verse)
                    dismiss()
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
            .navigationTitle("Notes")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                Button("Done") { dismiss() }
            }
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

extension HighlightColor {
    /// The desktop's colours (apps/desktop/assets/design-tokens.json).
    private var rgb: (swatch: UInt32, light: UInt32, dark: UInt32) {
        switch self {
        case .yellow: (0xEBC24A, 0xFBE7A1, 0x584A1E)
        case .green: (0x6DBE7E, 0xD3ECCB, 0x27452F)
        case .blue: (0x5B9BE6, 0xD4E4FA, 0x213A5C)
        case .pink: (0xE88AAE, 0xF8D5E1, 0x56293B)
        case .purple: (0xA58BE0, 0xE3DAF6, 0x3B325E)
        case .orange: (0xEE9A55, 0xFADDC2, 0x5A3A22)
        }
    }

    var swatch: Color { Color(UIColor(rgb: rgb.swatch)) }

    /// Behind highlighted text, in light and dark mode.
    var background: Color {
        let (light, dark) = (UIColor(rgb: rgb.light), UIColor(rgb: rgb.dark))
        return Color(UIColor { $0.userInterfaceStyle == .dark ? dark : light })
    }
}

private extension UIColor {
    convenience init(rgb: UInt32) {
        self.init(red: CGFloat(rgb >> 16 & 0xFF) / 255, green: CGFloat(rgb >> 8 & 0xFF) / 255,
                  blue: CGFloat(rgb & 0xFF) / 255, alpha: 1)
    }
}
