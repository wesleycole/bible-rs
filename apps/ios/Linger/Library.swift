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
    private(set) var error: String?
    private var sync: LibrarySync?
    private let relay = Relay()

    init() {
        do {
            study = try Study(dataDir: dataDirectory(), observer: relay)
        } catch {
            study = nil
            self.error = error.localizedDescription
        }
        relay.library = self
    }

    /// The marks on a chapter's verses, by verse number.
    func marks(book: UInt8, chapter: UInt8) -> [UInt8: VerseMarks] {
        _ = generation
        let marks = (try? study?.chapterMarks(book: book, chapter: chapter)) ?? []
        return Dictionary(uniqueKeysWithValues: marks.map { ($0.verse, $0) })
    }

    /// Syncs while Clerk has a user; stops (keeping the cache and what's waiting) when it hasn't.
    func signedIn(_ signedIn: Bool) {
        guard let study else { return }
        if !signedIn {
            sync = nil
        } else if sync == nil {
            do {
                sync = try LibrarySync.start(study: study, convexUrl: Config.convexURL, tokens: ClerkTokens(), observer: nil)
            } catch {
                self.error = error.localizedDescription
            }
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
            self.error = error.localizedDescription
        }
    }

    func dismissError() {
        error = nil
    }
}

/// Core calls its observer from its own threads.
private final class Relay: StudyObserver, @unchecked Sendable {
    weak var library: Library?

    func studyChanged(generation: UInt64) {
        Task { @MainActor in self.library?.generation = generation }
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
