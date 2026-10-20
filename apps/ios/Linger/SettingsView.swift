import ClerkKit
import LingerCore
import SwiftUI

/// Settings: the library's sync, and the account.
struct SettingsView: View {
    let library: Library
    @Environment(Clerk.self) private var clerk
    @Environment(\.dismiss) private var dismiss
    @State private var confirmingDelete = false

    var body: some View {
        NavigationStack {
            Form {
                Section("Library") {
                    LabeledContent("Sync", value: library.syncState.label)
                    if case .error = library.syncState {
                        Button("Retry", action: library.retry)
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
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                Button("Done") { dismiss() }
            }
            .task { await library.loadAccount() }
            .confirmationDialog("Delete your account?", isPresented: $confirmingDelete, titleVisibility: .visible) {
                Button("Delete account", role: .destructive) {
                    Task { await library.deleteAccount() }
                }
            } message: {
                Text("This deletes your library on Linger's servers and on this iPhone: notes, highlights, bookmarks and recordings. It can't be undone.")
            }
        }
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
