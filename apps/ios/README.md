# Linger for iOS

A SwiftUI app for iPhone and iPad on the same Rust core as the desktop app. It signs in
through Clerk (development instance) and reads any bundled translation chapter by chapter, with a
typed reference jump ("jn 3", "ps 23"). Study data and sync land next (M3a,
[docs/plans/m3-ios-app.md](../../docs/plans/m3-ios-app.md)); Config.swift says where it signs in
and syncs.

```
apps/ios/
├── project.yml        XcodeGen spec → Linger.xcodeproj (generated, not checked in)
├── Linger/            the app: LingerApp, ReaderModel, ReaderView
└── LingerCore/         Swift package wrapping linger-core
    ├── Package.swift
    ├── LingerFFI.xcframework           built by scripts/build-ios-core.sh (not checked in)
    ├── Sources/LingerCore/
    │   ├── LingerCore.swift            Swift helpers
    │   └── Generated/linger_ffi.swift  generated bindings (not checked in)
    └── Tests/LingerCoreTests/          calls Rust through the bindings
```

The bindings come from [`crates/ffi`](../../crates/ffi/src/lib.rs). Its interface is small:
`BibleLibrary` (translations, `chapter`, `search`), plus `books()`, `parseReference`,
`suggestReferences` and `formatReference`. Books are 0-based (Genesis 0, John 42); chapters and
verses are 1-based.

## Requirements

- macOS on Apple silicon with Xcode 16 or later
- Rust from the repository's toolchain (`rustup` installs it on first use)
- [XcodeGen](https://github.com/yonaskolb/XcodeGen): `brew install xcodegen`

## Build and run

From the repository root:

```sh
scripts/build-ios-core.sh                   # Rust → LingerFFI.xcframework + Swift bindings
swift test --package-path apps/ios/LingerCore  # the bindings' tests, on macOS
cd apps/ios && xcodegen && open Linger.xcodeproj
```

Then pick a simulator and run. The project signs with the Linger team (`66W3WWYP3B`) and the
existing bundle ID `com.wesleycole.linger`, so device builds and TestFlight uploads continue the
previous Linger app. Raise `CURRENT_PROJECT_VERSION` in `project.yml` before each upload.

Run `scripts/build-ios-core.sh` again after changing `crates/core` or `crates/ffi`;
`PROFILE=dev` makes an unoptimized build that compiles faster.

## Not done yet

- Sermon recording (AVFoundation, background audio, microphone permission) and transcription.
- API keys for online translations are stored in a file in the app's sandbox; on iOS they should
  move to the Keychain, as on macOS (`crates/core/src/secrets.rs`).
- The app doesn't use `Study` and `LibrarySync` from the bindings yet.
- Simulator builds for Intel Macs (an `x86_64-apple-ios` slice).
