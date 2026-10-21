import LingerCore
import StoreKit
import StoreKitTest
import XCTest

/// A purchase of Pro reaches the account: Xcode's StoreKit configuration sells pro.yearly, the proxy
/// verifies the transaction, and a refund takes Pro away again. Runs inside the app, the only
/// process storekitd lets use a StoreKit test session. Needs `pnpm dev` (APPLE_ENVIRONMENT=Xcode)
/// on 127.0.0.1:3000 and a token for the test user in LINGER_TEST_TOKEN:
///
///     TEST_RUNNER_LINGER_TEST_TOKEN="$(cd server && pnpm -s clerk-test-token --oauth)" xcodebuild test …
final class PurchaseTests: XCTestCase {
    func testBuyingProAndRefundingIt() async throws {
        guard let token = ProcessInfo.processInfo.environment["LINGER_TEST_TOKEN"], !token.isEmpty else {
            throw XCTSkip("needs LINGER_TEST_TOKEN and pnpm dev")
        }
        let account = Account(baseUrl: "http://127.0.0.1:3000", tokens: Token(token))
        let before: AccountInfo
        do { before = try account.me() } catch { throw XCTSkip("the proxy isn't reachable: \(error)") }
        XCTAssertFalse(before.pro, "the test user should start on Free")

        let session = try SKTestSession(contentsOf: URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Linger.storekit"))
        session.disableDialogs = true
        session.clearTransactions()
        // Even if an assertion fails, refund what was bought and tell the proxy, so the user ends Free.
        addTeardownBlock {
            if (try? account.me().pro) == true, let jws = try? await Self.refund(session) {
                _ = try? account.attachAppleTransaction(jws: jws)
            }
        }

        let products = try await Product.products(for: ["pro.yearly"])
        let yearly = try XCTUnwrap(products.first)
        let accountToken = try XCTUnwrap(before.appAccountToken.flatMap(UUID.init(uuidString:)))
        guard case let .success(bought) = try await yearly.purchase(options: [.appAccountToken(accountToken)]) else {
            return XCTFail("the purchase didn't succeed")
        }
        let pro = try account.attachAppleTransaction(jws: bought.jwsRepresentation)
        XCTAssertTrue(pro.pro)
        XCTAssertEqual(pro.source, "app_store")
        XCTAssertNotNil(pro.renewsAt)

        let refunded = try await Self.refund(session)
        XCTAssertFalse(try account.attachAppleTransaction(jws: refunded).pro)
    }

    /// Refunds every purchase in the session; returns pro.yearly's transaction once it shows the refund.
    private static func refund(_ session: SKTestSession) async throws -> String {
        for t in session.allTransactions() { try session.refundTransaction(identifier: t.identifier) }
        for _ in 0..<50 {
            if let latest = await Transaction.latest(for: "pro.yearly"), latest.unsafePayloadValue.revocationDate != nil {
                return latest.jwsRepresentation
            }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        return try XCTUnwrap(nil as String?, "the refund didn't reach the transaction")
    }
}

private final class Token: TokenProvider, @unchecked Sendable {
    let value: String
    init(_ value: String) { self.value = value }
    func token() -> String? { value }
    func refresh() -> String? { value }
}
