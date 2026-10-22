import ClerkKit
import LingerCore
import StoreKit
import SwiftUI

/// The Settings tab: the library's sync, and the account.
struct SettingsView: View {
    let library: Library
    @Environment(Clerk.self) private var clerk
    @State private var confirmingDelete = false
    @State private var showingPro = false
    @State private var managing = false

    var body: some View {
        NavigationStack {
            Form {
                Section("Library") {
                    if library.consent == false {
                        LabeledContent("Sync", value: "Only on this iPhone")
                        Button("Turn On Sync") { library.answerConsent(nil) }
                    } else {
                        LabeledContent("Sync", value: library.syncState.label)
                        if case .error = library.syncState {
                            Button("Retry", action: library.retry)
                        }
                    }
                    if let error = library.error {
                        Text(error).foregroundStyle(.red)
                        Button("Dismiss", action: library.dismissError)
                    }
                }
                Section("Account") {
                    LabeledContent("Email", value: clerk.user?.primaryEmailAddress?.emailAddress ?? "")
                    if let account = library.account {
                        LabeledContent("Plan", value: account.pro ? "Pro" : "Free")
                        if account.pro {
                            if let until = account.renewsAt.flatMap(date) {
                                LabeledContent("Paid through", value: until.formatted(date: .abbreviated, time: .omitted))
                            }
                            if account.source == "app_store" {
                                Button("Manage Subscription") { managing = true }
                            } else if account.source == "stripe" {
                                Text("Bought on the Mac: manage it in Linger there.").foregroundStyle(.secondary)
                            }
                        } else {
                            Button("Upgrade to Pro") { showingPro = true }
                        }
                        LabeledContent("Questions left", value: "\(account.questionsLeft)")
                        LabeledContent("Sermon hours left", value: account.sermonHoursLeft.formatted(.number.precision(.fractionLength(0...1))))
                        if let used = account.storageUsed, let limit = account.storageLimit {
                            LabeledContent("Storage", value: "\(bytes(used)) of \(bytes(limit))")
                        }
                    } else if library.error == nil {
                        ProgressView()
                    }
                    Button("Sign out") {
                        Task { await library.signOut() }
                    }
                    Button("Delete account", role: .destructive) { confirmingDelete = true }
                }
            }
            .navigationTitle("Settings")
            .task { await library.loadAccount() }
            .sheet(isPresented: $showingPro) { ProSheet(library: library) }
            .manageSubscriptionsSheet(isPresented: $managing)
            .confirmationDialog("Delete your account?", isPresented: $confirmingDelete, titleVisibility: .visible) {
                Button("Delete account", role: .destructive) {
                    Task { await library.deleteAccount() }
                }
            } message: {
                Text("This deletes your library on Linger's servers and on this iPhone: notes, highlights, bookmarks and recordings. It can't be undone.")
            }
        }
    }

    private func date(_ iso: String) -> Date? {
        let format = ISO8601DateFormatter()
        format.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return format.date(from: iso)
    }

    private func bytes(_ n: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(n), countStyle: .file)
    }
}

extension SyncState {
    /// What the desktop's sidebar footer says.
    var label: String {
        switch self {
        case .signedOut: "Signed out"
        case .synced: "Synced"
        case let .syncing(pending): pending == 0 ? "Syncing…" : "Syncing · \(waiting(pending))"
        case let .offline(pending): pending == 0 ? "Offline" : "Offline · \(waiting(pending))"
        case let .error(message): message
        }
    }

    private func waiting(_ n: UInt32) -> String {
        "\(n) change\(n == 1 ? "" : "s") waiting"
    }
}
