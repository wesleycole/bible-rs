//! The privacy promises the apps show, worded once (docs/plans/backend.md, "Privacy and
//! consent"), so every app's consent sheet says the same.

/// The library consent sheet: shown once, signed in, before the library syncs.
pub const LIBRARY_CONSENT_TITLE: &str = "Your library, on all your devices";
pub const LIBRARY_CONSENT_POINTS: [&str; 3] = [
    "Your notes, highlights, bookmarks and Recents are stored in Linger's cloud, so they're on all \
     your devices.",
    "Photos you add are read by Anthropic's Claude, and recordings are transcribed by AssemblyAI. \
     Neither keeps them or trains on them.",
    "You can export your library or delete it with your account at any time.",
];
