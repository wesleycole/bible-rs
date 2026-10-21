import LingerCore
import SwiftUI

/// Recents: saved Ask conversations, synced from the account's other devices. Read-only until
/// Ask comes to the iPhone; a verse citation opens its chapter.
struct RecentsList: View {
    let library: Library
    let open: (Reference) -> Void

    var body: some View {
        let _ = library.generation
        let chats = library.study?.chats() ?? []
        List(chats, id: \.id) { chat in
            NavigationLink {
                Conversation(library: library, chat: chat, open: open)
            } label: {
                VStack(alignment: .leading, spacing: 4) {
                    Text(chat.title).lineLimit(2)
                    Text(Date(timeIntervalSince1970: TimeInterval(chat.updated)), style: .date)
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .overlay {
            if chats.isEmpty {
                ContentUnavailableView("No conversations yet", systemImage: "bubble.left.and.text.bubble.right",
                                       description: Text("Questions you ask Linger on your Mac show up here."))
            }
        }
        .navigationTitle("Recents")
        .navigationBarTitleDisplayMode(.inline)
    }
}

private struct Conversation: View {
    let library: Library
    let chat: ChatSummary
    let open: (Reference) -> Void

    var body: some View {
        let _ = library.generation
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if let messages = library.study?.chat(id: chat.id) {
                    ForEach(Array(messages.enumerated()), id: \.offset) { _, message in
                        if message.fromUser {
                            Text(message.text).bold()
                        } else {
                            answer(message.text)
                        }
                    }
                } else {
                    ProgressView().frame(maxWidth: .infinity)
                }
            }
            .padding()
        }
        .navigationTitle(chat.title)
        .navigationBarTitleDisplayMode(.inline)
    }

    /// The answer's text with its verse citations as links to their chapters.
    private func answer(_ text: String) -> some View {
        var out = AttributedString()
        for segment in citationSegments(text: text) {
            switch segment {
            case let .text(text):
                out += AttributedString(text)
            case let .verse(reference):
                var link = AttributedString(reference.display)
                link.link = URL(string: "linger-verse:\(reference.book):\(reference.chapter)")
                out += link
            case .sermon:
                out += AttributedString("(a sermon)")
            case .item:
                out += AttributedString("(from your library)")
            }
        }
        return Text(out).environment(\.openURL, OpenURLAction { url in
            let parts = url.absoluteString.dropFirst("linger-verse:".count).split(separator: ":")
            guard parts.count == 2, let book = UInt8(parts[0]), let chapter = UInt8(parts[1]) else { return .discarded }
            open(Reference(book: book, chapter: chapter, verse: nil, verseEnd: nil))
            return .handled
        })
    }
}
