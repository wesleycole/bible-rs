import LingerCore
import SwiftUI
import UIKit

/// Every colour the app draws: the Linger theme's light and dark palettes, value for value from the
/// desktop's apps/desktop/assets/design-tokens.json (`color.light`, `color.dark`,
/// `color.highlight`). Each colour follows the system's light or dark appearance.
///
/// The six themes are moving into linger-core with an FFI call that returns a theme's palette;
/// when it lands, this file reads from it and nothing else changes.
enum Theme {
    // Surfaces
    static let page = Color(light: "#FFFFFF", dark: "#161618")
    static let grouped = Color(light: "#F2F2F4", dark: "#222225")
    static let surface = Color(light: "#FFFFFF", dark: "#26262A")
    static let fill = Color(light: "#1C1D210D", dark: "#FFFFFF0F")
    static let fillStrong = Color(light: "#1C1D211A", dark: "#FFFFFF1C")
    static let separator = Color(light: "#E4E4E8", dark: "#2E2E33")
    static let border = Color(light: "#1C1D211F", dark: "#FFFFFF1F")

    // Text
    static let ink = Color(light: "#1C1D21", dark: "#E6E3DC")
    static let ink2 = Color(light: "#6E6F76", dark: "#9C9CA3")
    static let ink3 = Color(light: "#A0A1A8", dark: "#66666D")

    // The accent, used sparingly: verse numbers, links, the current chapter.
    static let accent = Color(light: "#2D4A9E", dark: "#8EA6F2")
    static let onAccent = Color(light: "#FFFFFF", dark: "#111526")
    static let accentSoft = Color(light: "#E4E9F6", dark: "#263156")
    static let danger = Color(light: "#C4372C", dark: "#F07A6E")

    /// Liquid Glass tinted toward the desktop's glass colour.
    private static let glassTint = Color(light: "#FFFFFFE0", dark: "#303034E0")
    static var glass: Glass { .regular.tint(glassTint.opacity(0.55)) }
    static var glassInteractive: Glass { glass.interactive() }
}

extension HighlightColor {
    /// The verse row's five, in order; orange is the desktop's sixth.
    static let row: [HighlightColor] = [.yellow, .green, .blue, .pink, .purple]

    private var hex: (swatch: String, light: String, dark: String) {
        switch self {
        case .yellow: ("#EBC24A", "#FBE7A1", "#584A1E")
        case .green: ("#6DBE7E", "#D3ECCB", "#27452F")
        case .blue: ("#5B9BE6", "#D4E4FA", "#213A5C")
        case .pink: ("#E88AAE", "#F8D5E1", "#56293B")
        case .purple: ("#A58BE0", "#E3DAF6", "#3B325E")
        case .orange: ("#EE9A55", "#FADDC2", "#5A3A22")
        }
    }

    /// The saturated dot.
    var swatch: Color { Color(light: hex.swatch, dark: hex.swatch) }

    /// Behind highlighted text.
    var background: Color { Color(light: hex.light, dark: hex.dark) }
}

extension Color {
    /// `#RRGGBB` or `#RRGGBBAA` in sRGB, by appearance.
    init(light: String, dark: String) {
        let (light, dark) = (UIColor(hex: light), UIColor(hex: dark))
        self.init(UIColor { $0.userInterfaceStyle == .dark ? dark : light })
    }
}

private extension UIColor {
    convenience init(hex: String) {
        let digits = hex.dropFirst()
        let v = UInt64(digits, radix: 16) ?? 0
        let rgba = digits.count == 8 ? v : v << 8 | 0xFF
        self.init(red: CGFloat(rgba >> 24 & 0xFF) / 255, green: CGFloat(rgba >> 16 & 0xFF) / 255,
                  blue: CGFloat(rgba >> 8 & 0xFF) / 255, alpha: CGFloat(rgba & 0xFF) / 255)
    }
}
