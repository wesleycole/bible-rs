import StoreKit
import SwiftUI

/// What Pro adds (docs/plans/backend.md, "Plans and limits"), its two prices from the App Store, and
/// Restore Purchases.
struct ProSheet: View {
    let library: Library
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Label("150 questions a month, with Deep study", systemImage: "text.bubble")
                    Label("10 hours of sermon transcripts a month", systemImage: "waveform")
                    Label("300 photos read a month", systemImage: "photo")
                    Label("25 GB for your library", systemImage: "externaldrive")
                }
                Section {
                    ForEach(store.products, id: \.id) { product in
                        Button {
                            Task { await store.buy(product) }
                        } label: {
                            LabeledContent(product.displayName, value: price(product))
                        }
                        .accessibilityIdentifier(product.id)
                    }
                    Button("Restore Purchases") {
                        Task { await store.restore() }
                    }
                } footer: {
                    Text("Renews automatically until you cancel it in your Apple Account settings.")
                }
                .disabled(store.busy)
                if let error = store.error {
                    Text(error).foregroundStyle(.red)
                }
            }
            .navigationTitle("Linger Pro")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                Button("Close") { dismiss() }
            }
            .task { await store.loadProducts() }
            .onChange(of: library.account?.pro) { _, pro in
                if pro == true { dismiss() }
            }
        }
    }

    private func price(_ product: Product) -> String {
        product.subscription?.subscriptionPeriod.unit == .year
            ? "\(product.displayPrice) a year" : "\(product.displayPrice) a month"
    }
}
