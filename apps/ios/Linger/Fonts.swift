import CoreText
import SwiftUI

/// The desktop's type: Literata for scripture and titles, Inter for interface text. Both are
/// variable fonts bundled in Fonts/ with their OFL licences.
enum Fonts {
    /// Makes the bundled fonts available by name; called once at launch.
    static func register() {
        for name in ["Inter", "Literata", "Literata-Italic"] {
            guard let url = Bundle.main.url(forResource: name, withExtension: "ttf") else {
                assertionFailure("\(name).ttf isn't in the app bundle")
                continue
            }
            CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
        }
    }

    static func serif(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .custom("Literata-Regular", size: size).weight(weight)
    }

    static func serifItalic(_ size: CGFloat) -> Font {
        .custom("Literata-Italic", size: size)
    }

    static func ui(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .custom("Inter-Regular", size: size).weight(weight)
    }
}
