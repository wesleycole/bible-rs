import ClerkKit
import SwiftUI

@main
struct LingerApp: App {
    @State private var model = ReaderModel()

    init() {
        Clerk.configure(publishableKey: Config.clerkPublishableKey)
    }

    var body: some Scene {
        WindowGroup {
            Root(model: model)
                .environment(Clerk.shared)
        }
    }
}

/// The reader once signed in; the sign-in screen until then.
private struct Root: View {
    @Environment(Clerk.self) private var clerk
    let model: ReaderModel

    var body: some View {
        if !clerk.isLoaded {
            ProgressView()
        } else if clerk.user == nil {
            SignInView()
        } else {
            ReaderView(model: model)
        }
    }
}
