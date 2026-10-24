import ClerkKit
import LingerCore
import SwiftUI

@main
struct LingerApp: App {
    @State private var model = ReaderModel()
    @State private var library: Library
    @State private var store: Store

    init() {
        Clerk.configure(publishableKey: Config.clerkPublishableKey)
        Fonts.register()
        let library = Library()
        _library = State(initialValue: library)
        _store = State(initialValue: Store(library: library))
    }

    var body: some Scene {
        WindowGroup {
            Root(model: model, library: library)
                .environment(Clerk.shared)
                .environment(store)
                .tint(Theme.accent)
        }
    }
}

/// The tabs once signed in; the sign-in screen until then.
private struct Root: View {
    @Environment(Clerk.self) private var clerk
    let model: ReaderModel
    let library: Library

    var body: some View {
        Group {
            if !clerk.isLoaded {
                ProgressView()
            } else if clerk.user == nil {
                SignInView()
            } else {
                Tabs(model: model, library: library)
            }
        }
        .task(id: clerk.user?.id) { library.signedIn(clerk.user != nil) }
        .sheet(isPresented: .constant(clerk.user != nil && library.consent == nil)) {
            ConsentSheet(library: library)
        }
    }
}

enum AppTab: Hashable {
    case bible, library, settings, ask
}

/// Bible, Library and Settings, with Ask in the separated round slot at the bar's trailing end.
private struct Tabs: View {
    let model: ReaderModel
    let library: Library
    @State private var ask: AskModel
    @State private var tab = AppTab.bible
    @State private var libraryPath: [ItemRoute] = []
    @State private var libraryFilter = LibraryFilter.all

    init(model: ReaderModel, library: Library) {
        self.model = model
        self.library = library
        _ask = State(initialValue: AskModel(library: library, bible: model.bible))
    }

    var body: some View {
        // The Ask slot isn't a place: choosing it opens a new question about what's on screen and
        // leaves the tab where it was.
        TabView(selection: Binding(get: { tab }, set: { if $0 == .ask { askAboutScreen() } else { tab = $0 } })) {
            Tab("Bible", systemImage: "book", value: .bible) {
                ReaderView(model: model, library: library, ask: ask.start,
                           openLibrary: { place in
                               libraryFilter = .chapter(place)
                               libraryPath = []
                               tab = .library
                           },
                           openItem: { open(item: $0, at: nil) })
            }
            Tab("Library", systemImage: "books.vertical", value: .library) {
                LibraryView(library: library, reader: model, path: $libraryPath, filter: $libraryFilter,
                            returning: ask.leftFrom != nil, openVerse: open(verse:), ask: { ask.start(model.askAbout(item: $0)) })
            }
            Tab("Settings", systemImage: "gearshape", value: .settings) {
                SettingsView(library: library)
            }
            // The search role puts Ask in the round slot; its label stays "Ask".
            Tab("Ask", systemImage: "bubble.left", value: .ask, role: .search) {
                Color.clear
            }
        }
        .tabBarMinimizeBehavior(.onScrollDown)
        // After a source is opened: brings the chat back as it was.
        .tabViewBottomAccessory(isEnabled: ask.leftFrom != nil) {
            ReturnBar(question: ask.leftFrom ?? "", action: ask.comeBack)
        }
        .sheet(isPresented: $ask.presented) {
            AskSheet(ask: ask, library: library, reader: model,
                     openVerse: { reference in
                         if model.backTo == nil, (reference.book, reference.chapter) != (model.book, model.chapter) {
                             model.backTo = model.place
                         }
                         open(verse: reference)
                     },
                     openItem: open(item:at:))
        }
        .onChange(of: ask.leftFrom) { _, left in if left == nil { model.backTo = nil } }
        // Settings shows what the study store refuses in place.
        .alert(library.error ?? "", isPresented: .constant(library.error != nil && tab != .settings)) {
            Button("OK", action: library.dismissError)
        }
    }

    /// The item page open in Library, or the verses selected or the chapter in Bible.
    private func askAboutScreen() {
        if tab == .library, let route = libraryPath.last, let item = try? library.study?.libraryItem(uid: route.uid) {
            ask.start(model.askAbout(item: item))
        } else {
            ask.start(model.askAbout())
        }
    }

    private func open(verse reference: Reference) {
        model.go(book: reference.book, chapter: reference.chapter, verse: reference.verse)
        tab = .bible
    }

    private func open(item uid: String, at atMs: UInt64?) {
        libraryPath = [ItemRoute(uid: uid, atMs: atMs)]
        tab = .library
    }
}

/// "‹ What does 8:28 really promise?" above the tab bar, while the chat waits.
private struct ReturnBar: View {
    let question: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                Image(systemName: "chevron.left").font(.system(size: 12, weight: .semibold))
                Text(question).font(Fonts.ui(15)).lineLimit(1)
            }
            .foregroundStyle(Theme.accent)
            .padding(.horizontal, 16)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Back to \(question)")
        .accessibilityIdentifier("return-bar")
    }
}
