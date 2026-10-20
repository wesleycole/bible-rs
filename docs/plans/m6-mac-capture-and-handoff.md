# M6: The Mac as a capture device, and handoff

Status: not started. Depends on: M4. Design: [library.md](library.md#handoff).

**Goal.** Reading on the Mac, take a photo of a book page with the iPhone straight into Linger, mark it,
and ask about it; move between devices and pick up where you were.

## Capture on the Mac

1. **Continuity Camera:** "Take Photo" and "Scan Documents" from the iPhone, in the File menu and the
   library's capture button. AppKit offers them to a responder that implements
   `validRequestor(forSendType:returnType:)` and `readSelection(from:)`; Linger adds one with objc2's
   `define_class!` next to `macos_window.rs` and places it in winit's responder chain (after the
   content view), and the menu item uses
   `NSMenuItem.importFromDeviceIdentifier`. The image arrives on the pasteboard and goes into the photo
   flow.
2. **Drag-and-drop and a file picker** on macOS and Linux (egui's dropped files; `rfd` for the picker).
3. **Image preparation** in core: decode (HEIC through ImageIO on macOS; JPEG and PNG everywhere with the
   `image` crate), orient, resize to 2,048 px, encode JPEG, make the thumbnail.
4. **Marks on the Mac:** an egui canvas over the photo for circle, underline and highlight with the
   trackpad or mouse, saved in Linger's stroke format and rendered into `photoMarked` by the shared
   renderer from M4.
5. Then the same submit, extraction and review as M4. Book pages stay behind the same release-build
   switch as on iOS until the lawyer answers ([README](README.md#roadmap)).

## Handoff

1. **Reading position:** add `positions` ([library.md](library.md#the-data-model)). Each device writes its
   place (debounced, at most every 10 seconds while reading). Opening Linger when another device read
   more recently shows a quiet "Continue Romans 8 from your iPhone" that jumps there.
2. **`NSUserActivity`** on macOS and iOS (`app.linger.reading`, with the verse and translation), so
   Linger appears in the Dock and App Switcher for Handoff.
3. **Reading plans:** add `planProgress`; the desktop's plans pages read and write it through the cache,
   and existing plans upload once. **iOS gains reading plans** (the catalogue in `plans.rs`, through the
   FFI): today's reading, marking a day read, and starting and stopping a plan.
4. **Settings that follow the user:** the `profile` record (the Ask translation and home church); everything
   else (theme, size, layout) stays per device.
5. **The purge chain** covers `positions`, `planProgress` and `profile`.

## Sign-in on macOS

Move the desktop's OAuth redirect to `ASWebAuthenticationSession` (`objc2-authentication-services`) with
the Associated Domains callback `https://auth.linger.app/oauth/callback` (the domain exists from M1) on
macOS 14.4 and later, and the `linger://oauth/callback` custom scheme before that
([backend.md](backend.md#clerk)). Linux keeps the loopback redirect.

## Tests

- A photo arriving by Continuity Camera (simulated by putting an image on the responder's pasteboard)
  reaches `review` with its marks.
- Marks drawn on the Mac and on the iPhone render to the same `photoMarked` for the same strokes.
- Position handoff: reading on device A, then opening device B, offers A's place; B's own later reading
  doesn't get overwritten by A's older position.

## Done when

- On the Mac, "Take Photo" on the iPhone puts the page into Linger, and after review Ask on the Mac cites
  it, all without touching the iPhone's Linger app.
- Reading position and plan progress follow the user between the Mac and the iPhone.
- Signing in on macOS no longer opens a browser tab.
