/// Where the app signs in, syncs and reaches Linger's proxy: Clerk's development instance, dev
/// Convex, and the proxy from `pnpm dev` on the Mac, which the simulator reaches at 127.0.0.1. The
/// production cut-over changes all three (docs/plans/m3-ios-app.md).
enum Config {
    static let clerkPublishableKey = "pk_test_ZWxlZ2FudC1taW5rLTY3LmNsZXJrLmFjY291bnRzLmRldiQ"
    static let apiURL = "http://127.0.0.1:3000"
    static let convexURL = "https://adorable-porcupine-835.convex.cloud"
}
