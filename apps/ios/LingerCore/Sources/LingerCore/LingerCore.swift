// Swift-side helpers over the generated bindings (Generated/linger_ffi.swift), which define
// BibleLibrary, the records it returns and the free functions books(), parseReference(…) etc.

import Foundation

/// Application Support/linger: translations and study data, the same layout the desktop app uses.
public func dataDirectory() throws -> String {
    let dir = try FileManager.default
        .url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        .appendingPathComponent("linger", isDirectory: true)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir.path
}

extension BibleLibrary {
    public static func openDefault(observer: LibraryObserver? = nil) throws -> BibleLibrary {
        BibleLibrary(dataDir: try dataDirectory(), observer: observer)
    }
}

extension Reference {
    /// "John 3:16". Empty for a reference outside the canon.
    public var display: String {
        (try? formatReference(reference: self)) ?? ""
    }
}
