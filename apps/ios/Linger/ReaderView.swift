import LingerCore
import SwiftUI

struct ReaderView: View {
    @Bindable var model: ReaderModel
    @State private var jump = ""

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
                await model.load()
            }
        }
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
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(verses, id: \.number) { verse in
                        VerseRow(verse: verse)
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
        ToolbarItemGroup(placement: .bottomBar) {
            Button("Previous chapter", systemImage: "chevron.left", action: model.previousChapter)
                .disabled(!model.canGoBack)
            Spacer()
            Picker("Translation", selection: $model.translation) {
                ForEach(model.translations, id: \.id) { t in
                    Text(t.abbrev).tag(t.id)
                }
            }
            Spacer()
            Button("Next chapter", systemImage: "chevron.right", action: model.nextChapter)
                .disabled(!model.canGoForward)
        }
    }
}

private struct VerseRow: View {
    let verse: Verse

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(verse.headings, id: \.self) { heading in
                Text(heading).font(.headline).padding(.top, 12)
            }
            (Text(number).font(.caption2).foregroundStyle(.secondary).baselineOffset(6)
                + Text(" " + verse.text))
                .font(.system(.body, design: .serif))
                .lineSpacing(5)
        }
    }

    private var number: String {
        verse.numberEnd > verse.number ? "\(verse.number)–\(verse.numberEnd)" : "\(verse.number)"
    }
}
