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
    @State private var tab = AppTab.bible

    var body: some View {
        TabView(selection: $tab) {
            Tab("Bible", systemImage: "book", value: .bible) {
                ReaderView(model: model, library: library)
            }
            Tab("Library", systemImage: "books.vertical", value: .library) {
                NotesList(library: library) { open(book: $0.book, chapter: $0.chapter) }
            }
            Tab("Settings", systemImage: "gearshape", value: .settings) {
                SettingsView(library: library)
            }
            // The search role puts Ask in the round slot; its label stays "Ask". Recents until Ask
            // comes to the iPhone.
            Tab("Ask", systemImage: "bubble.left", value: .ask, role: .search) {
                NavigationStack {
                    RecentsList(library: library) { open(book: $0.book, chapter: $0.chapter) }
                }
            }
        }
        .tabBarMinimizeBehavior(.onScrollDown)
        // Settings shows what the study store refuses in place.
        .alert(library.error ?? "", isPresented: .constant(library.error != nil && tab != .settings)) {
            Button("OK", action: library.dismissError)
        }
    }

    private func open(book: UInt8, chapter: UInt8) {
        model.go(book: book, chapter: chapter)
        tab = .bible
    }
}
