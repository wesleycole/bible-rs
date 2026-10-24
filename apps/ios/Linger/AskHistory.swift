import LingerCore
import SwiftUI

/// Ask's past chats, from the clock in its header: grouped by day, newest first, with search.
/// Those from the account's other devices are here too.
struct AskHistory: View {
    let library: Library
    let open: (Int64) -> Void
    @State private var query = ""
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let _ = library.generation
        let chats = (library.study?.chats() ?? []).filter {
            query.isEmpty || $0.title.localizedCaseInsensitiveContains(query)
        }
        NavigationStack {
            List {
                ForEach(days(chats), id: \.0) { day, chats in
                    Section(day) {
                        ForEach(chats, id: \.id) { chat in
                            Button { open(chat.id) } label: {
                                Text(chat.title).font(Fonts.ui(16)).foregroundStyle(Theme.ink).lineLimit(2)
                            }
                            .accessibilityIdentifier("history-\(chat.id)")
                        }
                    }
                }
            }
            .listStyle(.plain)
            .searchable(text: $query, prompt: "Search conversations")
            .overlay {
                if chats.isEmpty {
                    ContentUnavailableView(query.isEmpty ? "No conversations yet" : "No matches",
                                           systemImage: "bubble.left.and.text.bubble.right",
                                           description: Text(query.isEmpty ? "Questions you ask show up here, from all your devices." : ""))
                }
            }
            .navigationTitle("History")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { Button("Done") { dismiss() } }
        }
    }

    /// "Today", "Yesterday", then dates, each with its chats.
    private func days(_ chats: [ChatSummary]) -> [(String, [ChatSummary])] {
        var out: [(String, [ChatSummary])] = []
        for chat in chats {
            let day = dayName(Date(timeIntervalSince1970: TimeInterval(chat.updated)))
            if out.last?.0 == day { out[out.count - 1].1.append(chat) } else { out.append((day, [chat])) }
        }
        return out
    }
}

/// "Today", "Yesterday", or "Oct 12" (with the year when it isn't this one).
func dayName(_ date: Date) -> String {
    let calendar = Calendar.current
    if calendar.isDateInToday(date) { return "Today" }
    if calendar.isDateInYesterday(date) { return "Yesterday" }
    let thisYear = calendar.isDate(date, equalTo: .now, toGranularity: .year)
    return date.formatted(thisYear ? .dateTime.month(.abbreviated).day() : .dateTime.month(.abbreviated).day().year())
}
