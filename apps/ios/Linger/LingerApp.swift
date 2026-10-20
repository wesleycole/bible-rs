import ClerkKit
import SwiftUI

@main
struct LingerApp: App {
    @State private var model = ReaderModel()
    @State private var library = Library()

    init() {
        Clerk.configure(publishableKey: Config.clerkPublishableKey)
    }

    var body: some Scene {
        WindowGroup {
            Root(model: model, library: library)
                .environment(Clerk.shared)
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
    }
}
