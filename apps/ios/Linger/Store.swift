import Foundation
import LingerCore
import Observation
import StoreKit

/// Linger Pro through the App Store. Swift decides nothing here: every transaction's JWS goes to the
/// proxy, which verifies it and says what the plan is; a transaction is finished once it has.
@MainActor
@Observable
final class Store {
    static let productIDs = ["pro.yearly", "pro.monthly"]

    private(set) var products: [Product] = []
    private(set) var busy = false
    private(set) var error: String?
    private let library: Library

    init(library: Library) {
        self.library = library
        // Renewals, refunds, Ask to Buy and purchases made elsewhere, plus any transaction a
        // previous launch couldn't hand over. What fails stays unfinished and comes back next
        // launch, so it isn't shown.
        Task {
            for await update in Transaction.updates {
                _ = try? await self.attach(update)
            }
        }
    }

    func loadProducts() async {
        guard products.isEmpty else { return }
        do {
            products = try await Product.products(for: Self.productIDs)
                .sorted { Self.productIDs.firstIndex(of: $0.id)! < Self.productIDs.firstIndex(of: $1.id)! }
        } catch {
            self.error = error.localizedDescription
        }
    }

    func buy(_ product: Product) async {
        // Ties the purchase to this Linger account, so another account on the same Apple ID can't
        // claim it.
        guard let token = library.account?.appAccountToken.flatMap(UUID.init(uuidString:)) else {
            error = "Your account hasn't loaded yet. Try again in a moment."
            return
        }
        await run {
            if case let .success(transaction) = try await product.purchase(options: [.appAccountToken(token)]) {
                try await self.attach(transaction)
            }
        }
    }

    func restore() async {
        await run {
            try await AppStore.sync()
            for await transaction in Transaction.currentEntitlements {
                try await self.attach(transaction)
            }
        }
    }

    private func run(_ work: () async throws -> Void) async {
        busy = true
        error = nil
        do {
            try await work()
        } catch {
            self.error = message(of: error)
        }
        busy = false
    }

    private func attach(_ transaction: VerificationResult<Transaction>) async throws {
        try await library.attachAppleTransaction(transaction.jwsRepresentation)
        await transaction.unsafePayloadValue.finish()
    }
}
