import LingerCore
import SwiftUI

/// What Library lists: everything, one kind, the verses marked, or the items about a chapter
/// (from the reader's "3 in your library"). All, Yours and Reference join when M7 brings the
/// reference works; until then everything is yours, so Yours would repeat All.
enum LibraryFilter: Hashable {
    case all
    case sermons, notes, photos
    case marks
    case chapter(Reference)

    var kinds: [ItemKind]? {
        switch self {
        case .sermons: [.sermon]
        case .notes: [.note]
        case .photos: [.handwritten, .page, .slide]
        default: nil
        }
    }
}

/// The Library tab: a large title, search, the capture row and the filters; then what's about the
/// chapter being read and what's recent by day.
struct LibraryView: View {
    let library: Library
    let reader: ReaderModel
    @Binding var path: [ItemRoute]
    @Binding var filter: LibraryFilter
    /// Ask's return bar shows.
    let returning: Bool
    let openVerse: (Reference) -> Void
    let ask: (LingerCore.LibraryItem) -> Void
    @State private var query = ""
    @State private var hits: [LibraryHit] = []
    @State private var searched = ""
    @State private var writing = false

    var body: some View {
        let _ = library.generation
        let all = (try? library.study?.libraryItems(kinds: nil)) ?? []
        NavigationStack(path: $path) {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    header(count: all.count)
                    searchField.padding(.top, 18)
                    if trimmed.isEmpty {
                        captureRow.padding(.top, 20)
                        filters(all).padding(.top, 18)
                        content(all)
                    } else {
                        searchResults
                    }
                }
                .frame(maxWidth: 640, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.bottom, 30)
                .frame(maxWidth: .infinity)
            }
            .scrollDismissesKeyboard(.immediately)
            .background(Theme.page)
            .toolbarVisibility(.hidden, for: .navigationBar)
            .navigationDestination(for: ItemRoute.self) { route in
                ItemPage(library: library, route: route, returning: returning, openVerse: openVerse, ask: ask)
            }
            .task(id: trimmed) { await search() }
        }
        .sheet(isPresented: $writing) { LibraryNoteSheet(library: library) }
    }

    private var trimmed: String { query.trimmingCharacters(in: .whitespaces) }

    // MARK: The top

    private func header(count: Int) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Library").font(Fonts.serif(40, weight: .medium)).foregroundStyle(Theme.ink)
            Text("\(count) yours").font(Fonts.ui(16)).foregroundStyle(Theme.ink2)
                .accessibilityIdentifier("library-count")
        }
        .padding(.top, 40)
    }

    private var searchField: some View {
        HStack(spacing: 10) {
            Image(systemName: "magnifyingglass").font(.system(size: 15, weight: .medium)).foregroundStyle(Theme.ink2)
            TextField("Search your library", text: $query)
                .font(Fonts.ui(17))
                .autocorrectionDisabled()
                .submitLabel(.search)
                .accessibilityIdentifier("library-search")
            if !query.isEmpty {
                Button("Clear", systemImage: "xmark.circle.fill") { query = "" }
                    .labelStyle(.iconOnly).foregroundStyle(Theme.ink3).buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 16)
        .frame(height: 48)
        .background(Theme.fill, in: Capsule())
    }

    /// How Library adds. Record (M5), Photo (M4) and Import join as each lands.
    private var captureRow: some View {
        HStack(spacing: 0) {
            Button { writing = true } label: {
                VStack(spacing: 6) {
                    Image(systemName: "square.and.pencil").font(.system(size: 24)).frame(height: 28)
                    Text("Note").font(Fonts.ui(14))
                }
                .foregroundStyle(Theme.ink)
                .frame(width: 84, height: 64)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("library-note")
            Spacer(minLength: 0)
        }
    }

    /// All, the kinds the library holds, Marks once a verse is marked, and the chapter when the
    /// reader opened Library on it.
    private func filters(_ all: [LingerCore.LibraryItem]) -> some View {
        let kinds = Set(all.map(\.kind))
        var chips: [(String, LibraryFilter)] = [("All", .all)]
        if case let .chapter(place) = filter { chips.append((place.display, filter)) }
        if kinds.contains(.sermon) { chips.append(("Sermons", .sermons)) }
        if kinds.contains(.note) { chips.append(("Notes", .notes)) }
        if !kinds.isDisjoint(with: [.handwritten, .page, .slide]) { chips.append(("Photos", .photos)) }
        if !(library.study?.markedVerses().isEmpty ?? true) { chips.append(("Marks", .marks)) }
        return ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(chips, id: \.0) { label, value in
                    let on = filter == value
                    Button { filter = value } label: {
                        Text(label)
                            .font(Fonts.ui(15, weight: on ? .medium : .regular))
                            .foregroundStyle(on ? Theme.onAccent : Theme.ink)
                            .padding(.horizontal, 16)
                            .frame(height: 36)
                            .background(on ? Theme.ink : Theme.fill, in: Capsule())
                            .contentShape(Capsule())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(on ? .isSelected : [])
                    .accessibilityIdentifier("filter-\(label)")
                }
            }
        }
        .scrollClipDisabled()
    }

    // MARK: The lists

    @ViewBuilder
    private func content(_ all: [LingerCore.LibraryItem]) -> some View {
        switch filter {
        case .all:
            let here = library.chapterLibrary(book: reader.book, chapter: reader.chapter)
            if !here.isEmpty {
                sectionHeader("On \(reader.title)")
                ForEach(here, id: \.uid) { row($0) }
            }
            if !all.isEmpty {
                sectionHeader("Recent")
                byDay(all)
            } else {
                Text("What you add shows here: notes now, and sermons and photos as they come to the iPhone.")
                    .font(Fonts.ui(14)).foregroundStyle(Theme.ink3).padding(.top, 28)
            }
        case let .chapter(place):
            let here = library.chapterLibrary(book: place.book, chapter: place.chapter)
            sectionHeader("On \(place.display)")
            ForEach(here, id: \.uid) { row($0) }
        case .marks:
            marks
        default:
            byDay(all.filter { filter.kinds?.contains($0.kind) ?? true }).padding(.top, 8)
        }
    }

    /// Items under their day, newest first.
    @ViewBuilder
    private func byDay(_ items: [LingerCore.LibraryItem]) -> some View {
        let days = items.reduce(into: [(String, [LingerCore.LibraryItem])]()) { out, item in
            let day = dayName(item.date)
            if out.last?.0 == day { out[out.count - 1].1.append(item) } else { out.append((day, [item])) }
        }
        ForEach(days, id: \.0) { day, items in
            Text(day).font(Fonts.ui(15)).foregroundStyle(Theme.ink2).padding(.top, 16).padding(.bottom, 2)
            ForEach(items, id: \.uid) { row($0) }
        }
    }

    /// Highlights and bookmarks, in canonical order; each opens its verse.
    @ViewBuilder
    private var marks: some View {
        let marked = library.study?.markedVerses() ?? []
        sectionHeader("Marks")
        ForEach(marked, id: \.reference) { mark in
            Button {
                openVerse(Reference(book: mark.verse.book, chapter: mark.verse.chapter, verse: mark.verse.verse, verseEnd: nil))
            } label: {
                HStack(spacing: 12) {
                    Circle().fill(mark.highlight?.swatch ?? .clear).frame(width: 14, height: 14)
                        .overlay { if mark.highlight == nil { Circle().strokeBorder(Theme.separator) } }
                    Text(mark.reference).font(Fonts.serif(18)).foregroundStyle(Theme.ink)
                    Spacer(minLength: 0)
                    if mark.bookmark {
                        Image(systemName: "bookmark.fill").font(.system(size: 12)).foregroundStyle(Theme.accent.opacity(0.6))
                            .accessibilityLabel("Bookmarked")
                    }
                }
                .frame(minHeight: 50)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .overlay(alignment: .bottom) { Hairline() }
        }
    }

    private func row(_ item: LingerCore.LibraryItem) -> some View {
        NavigationLink(value: ItemRoute(uid: item.uid)) { LibraryRow(item: item) }
            .buttonStyle(.plain)
            .accessibilityIdentifier("item-\(item.uid.prefix(8))")
    }

    // MARK: Search

    @ViewBuilder
    private var searchResults: some View {
        if searched == trimmed {
            if hits.isEmpty {
                Text(library.librarySync == nil ? "Turn on sync in Settings to search your library."
                                                : "Nothing in your library matches.")
                    .font(Fonts.ui(14)).foregroundStyle(Theme.ink3).padding(.top, 24)
            } else {
                sectionHeader("In your library")
                ForEach(Array(hits.enumerated()), id: \.offset) { _, hit in
                    NavigationLink(value: ItemRoute(uid: hit.uid, atMs: hit.startMs)) {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(hit.title).font(Fonts.serif(17)).foregroundStyle(Theme.ink).lineLimit(1)
                            Text(hit.text).font(Fonts.ui(14)).foregroundStyle(Theme.ink2).lineLimit(3)
                                .multilineTextAlignment(.leading)
                            Text([hit.kind.origin, hit.startMs.map(clock)].compactMap { $0 }.joined(separator: " · "))
                                .font(Fonts.ui(12)).foregroundStyle(Theme.ink3)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.vertical, 12)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .overlay(alignment: .bottom) { Hairline() }
                }
            }
        }
    }

    private func search() async {
        let q = trimmed
        guard !q.isEmpty else { return }
        try? await Task.sleep(for: .milliseconds(200))
        guard !Task.isCancelled else { return }
        let found = await library.search(q)
        guard !Task.isCancelled else { return }
        hits = found
        searched = q
    }

    private func sectionHeader(_ title: String) -> some View {
        Text(title.uppercased())
            .font(Fonts.ui(13, weight: .medium))
            .tracking(1.4)
            .foregroundStyle(Theme.ink2)
            .padding(.top, 30)
            .padding(.bottom, 6)
    }
}

/// An item as Library lists it: its kind's glyph (a ring while it's being worked on), its title in
/// serif, and a grey line saying whose it is and where it's from.
struct LibraryRow: View {
    let item: LingerCore.LibraryItem

    var body: some View {
        HStack(spacing: 16) {
            Group {
                if item.inProgress {
                    ProgressView().progressViewStyle(.circular)
                } else {
                    Image(systemName: item.kind.symbol).font(.system(size: 20, weight: .light))
                }
            }
            .foregroundStyle(Theme.ink2)
            .frame(width: 30)
            VStack(alignment: .leading, spacing: 3) {
                Text(item.title).font(Fonts.serif(19)).foregroundStyle(Theme.ink).lineLimit(1)
                Text(item.subtitle).font(Fonts.ui(14)).foregroundStyle(Theme.ink2).lineLimit(1)
            }
            Spacer(minLength: 0)
            Image(systemName: "chevron.right").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.ink3)
        }
        .frame(minHeight: 68)
        .contentShape(Rectangle())
        .overlay(alignment: .bottom) { Hairline() }
        .accessibilityElement(children: .combine)
    }
}

extension LingerCore.LibraryItem {
    var date: Date { Date(timeIntervalSince1970: TimeInterval(capturedAt) / 1000) }

    /// Not ready yet: recording, uploading, waiting or being transcribed.
    var inProgress: Bool { status != .ready && status != .error && status != .review }

    /// "Your sermon · Pastor Mike · Oct 12", "Your note · Romans 8:26", "Your recording ·
    /// Transcribing".
    var subtitle: String {
        var parts = [inProgress ? "Your recording" : kind.origin]
        switch status {
        case .recording: parts.append("Recording")
        case .uploading: parts.append("Uploading")
        case .waiting, .processing: parts.append("Transcribing")
        case .error: parts.append("Couldn't be transcribed")
        case .review, .ready:
            if let speaker { parts.append(speaker) } else if let anchor = anchors.first { parts.append(anchor.reference) }
            if kind == .sermon { parts.append(date.formatted(.dateTime.month(.abbreviated).day())) }
        }
        return parts.joined(separator: " · ")
    }
}

/// A note for the library from its capture row, anchored by the references written in it.
private struct LibraryNoteSheet: View {
    let library: Library
    @State private var text = ""
    @FocusState private var editing: Bool
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            TextField("Write a note… References like Romans 8:28 anchor it.", text: $text, axis: .vertical)
                .focused($editing)
                .font(Fonts.serif(18))
                .foregroundStyle(Theme.ink)
                .padding(.horizontal, 22)
                .padding(.top, 8)
                .frame(maxHeight: .infinity, alignment: .top)
                .background(Theme.page)
                .accessibilityIdentifier("library-note-field")
                .navigationTitle("Note")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel", systemImage: "xmark") { dismiss() }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Save") {
                            library.write { _ = try $0.saveNoteItem(text: text, ranges: []) }
                            dismiss()
                        }
                        .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .accessibilityIdentifier("library-note-save")
                    }
                }
        }
        .presentationDetents([.large])
        .presentationBackground(Theme.page)
        .interactiveDismissDisabled(!text.isEmpty)
        .onAppear { editing = true }
    }
}
