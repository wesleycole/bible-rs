import LingerCore
import SwiftUI

/// A library item Library opens, at a moment of its transcript when Ask cited one.
struct ItemRoute: Hashable {
    let uid: String
    var atMs: UInt64?
}

/// An item's page: whose it is and its title, the speaker line, the summary, the passages it's
/// anchored to (each opens in Bible), and its text: a sermon's timed transcript, a photo's words,
/// or a note. Back and Share on top; a floating "Ask about this sermon" bar at the foot.
struct ItemPage: View {
    let library: Library
    let route: ItemRoute
    /// Ask's return bar shows: the tab bar stays for it, and the tab bar's Ask asks about this
    /// item, so the page has no bar of its own.
    let returning: Bool
    let openVerse: (Reference) -> Void
    let ask: (LingerCore.LibraryItem) -> Void

    var body: some View {
        let _ = library.generation
        if let item = try? library.study?.libraryItem(uid: route.uid) {
            let text = try? library.study?.libraryItemText(uid: route.uid)
            page(item, text ?? nil)
        } else {
            ContentUnavailableView("This isn't in your library anymore", systemImage: "books.vertical")
                .background(Theme.page)
        }
    }

    private func page(_ item: LingerCore.LibraryItem, _ text: ItemText?) -> some View {
        let segments: [TranscriptSegment] = if case let .transcript(segments) = text { segments } else { [] }
        let cited = route.atMs.flatMap { at in segments.lastIndex { $0.startMs <= at } }
        return ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    Text(item.subtitle.components(separatedBy: " · ").first ?? item.kind.origin)
                        .font(Fonts.ui(14)).foregroundStyle(Theme.ink2)
                    Text(item.title)
                        .font(Fonts.serif(34, weight: .medium))
                        .foregroundStyle(Theme.ink)
                        .padding(.top, 2)
                        .accessibilityAddTraits(.isHeader)
                        .accessibilityIdentifier("item-title")
                    if let meta = meta(item, segments) {
                        Text(meta).font(Fonts.ui(15)).foregroundStyle(Theme.ink2).padding(.top, 6)
                    }
                    if let summary = item.summary, !summary.isEmpty {
                        section("Summary")
                        Text(summary).font(Fonts.serif(18)).lineSpacing(4).foregroundStyle(Theme.ink)
                    }
                    if !item.anchors.isEmpty {
                        section("Passages")
                        ForEach(Array(item.anchors.enumerated()), id: \.offset) { _, anchor in
                            Button { openVerse(reference(anchor)) } label: {
                                Text(anchor.reference).font(Fonts.serif(18)).foregroundStyle(Theme.accent)
                                    .frame(minHeight: 36, alignment: .leading)
                                    .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("item-passage-\(anchor.reference)")
                        }
                    }
                    if !segments.isEmpty {
                        section("Transcript")
                        ForEach(Array(segments.enumerated()), id: \.offset) { i, segment in
                            // Times are text, not play buttons: playback comes later.
                            HStack(alignment: .firstTextBaseline, spacing: 16) {
                                Text(clock(segment.startMs))
                                    .font(Fonts.ui(15).monospacedDigit())
                                    .foregroundStyle(Theme.ink2)
                                    .frame(width: 60, alignment: .leading)
                                Text(segment.text).font(Fonts.serif(17)).lineSpacing(3).foregroundStyle(Theme.ink)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                            .padding(.vertical, 6)
                            .padding(.horizontal, 8)
                            .background {
                                if i == cited { RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.accentSoft) }
                            }
                            .padding(.horizontal, -8)
                            .id(i)
                        }
                    } else if case let .plain(body) = text, !body.isEmpty {
                        if item.kind != .note { section("Text") }
                        Text(body).font(Fonts.serif(18)).lineSpacing(4).foregroundStyle(Theme.ink)
                            .padding(.top, item.kind == .note ? 22 : 0)
                            .textSelection(.enabled)
                            .accessibilityIdentifier("item-text")
                    } else if item.summary == nil, let excerpt = item.excerpt, !excerpt.isEmpty {
                        Text(excerpt).font(Fonts.serif(18)).lineSpacing(4).foregroundStyle(Theme.ink).padding(.top, 22)
                    }
                }
                .frame(maxWidth: 640, alignment: .leading)
                .padding(.horizontal, 22)
                .padding(.top, 4)
                .padding(.bottom, 30)
                .frame(maxWidth: .infinity)
            }
            .onAppear {
                if let cited { Task { proxy.scrollTo(cited, anchor: UnitPoint(x: 0.5, y: 0.2)) } }
            }
        }
        .background(Theme.page)
        .navigationTitle("")
        .navigationBarTitleDisplayMode(.inline)
        .toolbarVisibility(.visible, for: .navigationBar)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                ShareLink(item: shareText(item, text)) {
                    Image(systemName: "square.and.arrow.up")
                }
                .tint(Theme.ink)
                .accessibilityLabel("Share")
                .accessibilityIdentifier("item-share")
            }
        }
        .toolbarVisibility(returning ? .automatic : .hidden, for: .tabBar)
        .safeAreaBar(edge: .bottom, spacing: 0) {
            if !returning {
                AskBar(title: "Ask about this \(item.kind.noun)") { ask(item) }
                    .accessibilityIdentifier("item-ask")
            }
        }
    }

    /// "Pastor Mike · Grace Church · Oct 12 · 38 min"; a note's or photo's date.
    private func meta(_ item: LingerCore.LibraryItem, _ segments: [TranscriptSegment]) -> String? {
        var parts = [item.speaker, item.from].compactMap { $0 }
        parts.append(item.date.formatted(Calendar.current.isDate(item.date, equalTo: .now, toGranularity: .year)
            ? .dateTime.month(.abbreviated).day() : .dateTime.month(.abbreviated).day().year()))
        if let end = segments.last?.endMs, end >= 60_000 { parts.append("\(end / 60_000) min") }
        return parts.joined(separator: " · ")
    }

    private func section(_ title: String) -> some View {
        Text(title).font(Fonts.serif(24, weight: .medium)).foregroundStyle(Theme.ink)
            .padding(.top, 30).padding(.bottom, 8)
            .accessibilityAddTraits(.isHeader)
    }

    private func reference(_ anchor: ItemAnchor) -> Reference {
        let sameChapter = (anchor.from.book, anchor.from.chapter) == (anchor.to.book, anchor.to.chapter)
        return Reference(book: anchor.from.book, chapter: anchor.from.chapter, verse: anchor.from.verse,
                         verseEnd: sameChapter && anchor.to.verse != anchor.from.verse ? anchor.to.verse : nil)
    }

    /// The title, then the summary or the item's own words.
    private func shareText(_ item: LingerCore.LibraryItem, _ text: ItemText?) -> String {
        let body: String? = switch text {
        case let .plain(text): text
        case let .transcript(segments): item.summary ?? segments.map(\.text).joined(separator: " ")
        case nil: item.summary ?? item.excerpt
        }
        return [item.title, body].compactMap { $0 }.joined(separator: "\n\n")
    }
}
