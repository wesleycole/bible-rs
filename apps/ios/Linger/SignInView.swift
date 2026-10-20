import ClerkKitUI
import SwiftUI

/// The first screen: what signing in is for, then Clerk's own sign-in (email code or passkey, as
/// the instance allows).
struct SignInView: View {
    @State private var signingIn = false

    var body: some View {
        VStack(spacing: 16) {
            Spacer()
            Text("Linger")
                .font(.largeTitle.bold())
            Text("Your notes, highlights and sermons, kept in one library on your iPhone and your Mac.")
                .font(.title3)
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)
            Spacer()
            Button("Sign in") { signingIn = true }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
        }
        .padding(32)
        .frame(maxWidth: 480)
        .sheet(isPresented: $signingIn) {
            AuthView()
        }
    }
}
