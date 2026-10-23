import LingerCore
import SwiftUI

/// The items of the library about one verse, from the margin glyph: "Romans 8:28 ›", which opens
/// the verse page, then Your library, each row with an Ask button. Reference joins it with M7.
struct MarginPopover: View {
    let verse: VerseRef
    let items: [LingerCore.LibraryItem]
    let open: () -> Void
    let ask: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button(action: open) {
                HStack(spacing: 4) {
                    Text(verse.display).font(Fonts.serif(17, weight: .medium)).foregroundStyle(Theme.ink)
                    Image(systemName: "chevron.right").font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.ink2)
                    Spacer(minLength: 0)
                }
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("popover-header")
            .padding(.bottom, 6)
            if !items.isEmpty {
                Text("Your library").font(Fonts.ui(11, weight: .medium)).foregroundStyle(Theme.ink2)
                    .padding(.top, 8).padding(.bottom, 2)
                ForEach(items, id: \.uid) { item in
                    HStack(spacing: 10) {
                        ItemRow(item: item, compact: true)
                        Button(action: ask) {
                            Image(systemName: "bubble.left")
                                .font(.system(size: 12))
                                .foregroundStyle(Theme.accent)
                                .frame(width: 30, height: 30)
                                .overlay { Circle().strokeBorder(Theme.accent.opacity(0.3), lineWidth: 1) }
                                .frame(width: 44, height: 44)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Ask about \(item.title)")
                    }
                    .frame(minHeight: 44)
                }
            }
        }
        .padding(16)
        .frame(width: 310)
    }
}

/// A library item as a row: its kind's glyph, its title, and where it's anchored.
struct ItemRow: View {
    let item: LingerCore.LibraryItem
    /// The popover's smaller row, without the origin label.
    var compact = false

    var body: some View {
        HStack(spacing: compact ? 10 : 12) {
            Image(systemName: item.kind.symbol)
                .font(.system(size: compact ? 13 : 14))
                .foregroundStyle(Theme.ink2)
                .frame(width: compact ? 18 : 20)
            VStack(alignment: .leading, spacing: compact ? 1 : 2) {
                Text(item.title).font(Fonts.ui(compact ? 14 : 15)).foregroundStyle(Theme.ink).lineLimit(1)
                Text(compact ? anchors : "\(item.kind.origin) · \(anchors)")
                    .font(Fonts.ui(compact ? 11 : 12)).foregroundStyle(Theme.ink2).lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .combine)
    }

    private var anchors: String { item.anchors.map(\.reference).joined(separator: ", ") }
}

extension ItemKind {
    var symbol: String {
        switch self {
        case .sermon: "mic"
        case .handwritten, .page: "photo"
        case .slide: "rectangle.on.rectangle"
        case .note: "doc.text"
        }
    }

    /// "Your sermon": what it is and that it's yours, as Library labels it.
    var origin: String {
        switch self {
        case .sermon: "Your sermon"
        case .handwritten, .page: "Your photo"
        case .slide: "Your slide"
        case .note: "Your note"
        }
    }
}
