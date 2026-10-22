import ClerkKit
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
        }
    }
}

/// The reader once signed in; the sign-in screen until then.
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
                ReaderView(model: model, library: library)
            }
        }
        .task(id: clerk.user?.id) { library.signedIn(clerk.user != nil) }
        .sheet(isPresented: .constant(clerk.user != nil && library.consent == nil)) {
            ConsentSheet(library: library)
        }
    }
}
