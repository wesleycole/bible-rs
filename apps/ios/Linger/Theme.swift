import LingerCore
import SwiftUI
import UIKit

/// Every colour the app draws, from core's theme palettes (`themePalette`), the desktop's design
/// tokens. Each colour follows the system's light or dark appearance. The theme is Linger until
/// the synced `preferences` record and Settings › Appearance arrive.
enum Theme {
    private static let light = themePalette(theme: .linger, dark: false)
    private static let dark = themePalette(theme: .linger, dark: true)

    private static func color(_ role: (ThemePalette) -> UInt32) -> Color {
        let (l, d) = (UIColor(rgba: role(light)), UIColor(rgba: role(dark)))
        return Color(UIColor { $0.userInterfaceStyle == .dark ? d : l })
    }

    // Surfaces
    static let page = color(\.paper)
    static let grouped = color(\.sidebar)
    static let surface = color(\.surface)
    static let fill = color(\.fill)
    static let fillStrong = color(\.fillStrong)
    static let separator = color(\.separator)
    static let border = color(\.border)

    // Text
    static let ink = color(\.ink)
    static let ink2 = color(\.ink2)
    static let ink3 = color(\.ink3)

    // The accent, used sparingly: verse numbers, links, the current chapter.
    static let accent = color(\.accent)
    static let onAccent = color(\.onAccent)
    static let accentSoft = color(\.accentSoft)
    static let danger = color(\.danger)

    /// Liquid Glass tinted toward the theme's glass colour.
    private static let glassTint = color(\.glass)
    static var glass: Glass { .regular.tint(glassTint.opacity(0.55)) }
    static var glassInteractive: Glass { glass.interactive() }

    /// A highlight's dot and the fill behind highlighted text.
    fileprivate static func highlight(_ c: HighlightColor) -> (swatch: Color, background: Color) {
        let i = highlightColors().firstIndex(of: c) ?? 0
        return (color { $0.swatches[i] }, color { $0.highlights[i] })
    }
}

extension HighlightColor {
    /// The verse row's five, in order; orange is the desktop's sixth.
    static let row: [HighlightColor] = [.yellow, .green, .blue, .pink, .purple]

    /// The saturated dot.
    var swatch: Color { Theme.highlight(self).swatch }

    /// Behind highlighted text.
    var background: Color { Theme.highlight(self).background }
}

private extension UIColor {
    /// 0xRRGGBBAA, straight alpha, sRGB.
    convenience init(rgba: UInt32) {
        self.init(red: CGFloat(rgba >> 24 & 0xFF) / 255, green: CGFloat(rgba >> 16 & 0xFF) / 255,
                  blue: CGFloat(rgba >> 8 & 0xFF) / 255, alpha: CGFloat(rgba & 0xFF) / 255)
    }
}
