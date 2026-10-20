import ClerkKit
import Foundation
import LingerCore
import Observation

/// Highlights, bookmarks and notes (core's `Study`), synced with the account while signed in.
@MainActor
@Observable
final class Library {
    let study: Study?
    /// Moves when study data changes, here or from another device.
    fileprivate(set) var generation: UInt64 = 0
    fileprivate(set) var syncState: SyncState = .signedOut
    /// The account's plan and allowances, from the proxy.
    private(set) var account: AccountInfo?
    private(set) var error: String?
    fileprivate var sync: LibrarySync?
    private let relay = Relay()
    private let proxy = Account(baseUrl: Config.apiURL, tokens: ClerkTokens())

    init() {
        do {
            study = try Study(dataDir: dataDirectory(), observer: relay)
        } catch {
            study = nil
            self.error = message(of: error)
        }
        relay.library = self
    }

    /// The marks on a chapter's verses, by verse number.
    func marks(book: UInt8, chapter: UInt8) -> [UInt8: VerseMarks] {
        _ = generation
        let marks = (try? study?.chapterMarks(book: book, chapter: chapter)) ?? []
        return Dictionary(uniqueKeysWithValues: marks.map { ($0.verse, $0) })
    }

    /// Changes the study; what it refuses is shown.
    func write(_ change: (Study) throws -> Void) {
        guard let study else { return }
        do {
            try change(study)
        } catch {
            self.error = message(of: error)
        }
    }

    /// Syncs while Clerk has a user; stops (keeping the cache and what's waiting) when it hasn't.
    func signedIn(_ signedIn: Bool) {
        guard let study else { return }
        if !signedIn {
            sync = nil
            account = nil
            syncState = .signedOut
        } else if sync == nil {
            do {
                let sync = try LibrarySync.start(
                    study: study, convexUrl: Config.convexURL, tokens: ClerkTokens(), observer: relay)
                self.sync = sync
                syncState = sync.status()
            } catch {
                self.error = message(of: error)
            }
        }
    }

    func retry() {
        sync?.retry()
    }

    func loadAccount() async {
        let proxy = self.proxy
        switch await Task.detached(operation: { Result { try proxy.me() } }).value {
        case let .success(info): account = info
        case let .failure(error): self.error = message(of: error)
        }
    }

    /// Deletes the account on the server, then everything of it on this device. If the server
    /// refuses, the device keeps its data.
    func deleteAccount() async {
        let proxy = self.proxy
        if case let .failure(error) = await Task.detached(operation: { Result { try proxy.deleteAccount() } }).value {
            self.error = message(of: error)
            return
        }
        sync = nil
        do {
            try study?.clearLibrary()
            try await Clerk.shared.auth.signOut()
        } catch {
            self.error = message(of: error)
        }
    }

    /// Sends what's waiting, then forgets this account's library on the device and signs out.
    func signOut() async {
        if let sync {
            _ = await Task.detached { sync.waitIdle(timeoutMs: 10_000) }.value
            if let waiting = study?.pendingChanges(), waiting > 0 {
                error = "\(waiting) changes haven't reached Linger's server yet. Try again once you're online."
                return
            }
        }
        sync = nil
        do {
            try study?.clearLibrary()
            try await Clerk.shared.auth.signOut()
        } catch {
            self.error = message(of: error)
        }
    }

    func dismissError() {
        error = nil
    }
}

/// Core calls its observers from its own threads.
private final class Relay: StudyObserver, SyncObserver, @unchecked Sendable {
    weak var library: Library?

    func studyChanged(generation: UInt64) {
        Task { @MainActor in self.library?.generation = generation }
    }

    func syncChanged() {
        Task { @MainActor in
            guard let library = self.library, let sync = library.sync else { return }
            library.syncState = sync.status()
        }
    }
}

/// Clerk's session token for core, which asks from its own threads and waits for the answer.
private final class ClerkTokens: TokenProvider, @unchecked Sendable {
    func token() -> String? { fetch(skipCache: false) }
    func refresh() -> String? { fetch(skipCache: true) }

    private func fetch(skipCache: Bool) -> String? {
        // Clerk's calls run on the main actor, so waiting for one there would never end.
        precondition(!Thread.isMainThread, "core asks for tokens off the main thread")
        final class Answer: @unchecked Sendable { var token: String? }
        let (answer, done) = (Answer(), DispatchSemaphore(value: 0))
        Task { @MainActor in
            answer.token = try? await Clerk.shared.auth.getToken(.init(skipCache: skipCache))
            done.signal()
        }
        _ = done.wait(timeout: .now() + 20)
        return answer.token
    }
}
