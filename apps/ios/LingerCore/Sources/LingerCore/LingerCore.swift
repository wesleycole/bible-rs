// Swift-side helpers over the generated bindings (Generated/linger_ffi.swift), which define
// BibleLibrary, the records it returns and the free functions books(), parseReference(…) etc.

import Foundation

extension BibleLibrary {
    /// The library in Application Support/linger, the same layout the desktop app uses.
    public static func openDefault(observer: LibraryObserver? = nil) throws -> BibleLibrary {
        let dir = try FileManager.default
            .url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            .appendingPathComponent("linger", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return BibleLibrary(dataDir: dir.path, observer: observer)
    }
}

extension Reference {
    /// "John 3:16". Empty for a reference outside the canon.
    public var display: String {
        (try? formatReference(reference: self)) ?? ""
    }
}
