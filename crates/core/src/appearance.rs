//! Appearance, shared by the Mac and the iPhone: six themes, each a light and a dark palette, and
//! the colour scheme (System, Light or Dark) that picks which one shows. Both are synced as the
//! [`Preferences`](crate::records::Preferences) record.
//!
//! Every palette comes from the design's token file, `apps/desktop/assets/design-tokens.json`:
//! Linger is `color.light` and `color.dark` exactly, and Paper's light is `color.sepia`. Forest,
//! Oxblood, Plum and Graphite are Linger's neutrals with their own accent, and Paper's dark is
//! built from three warm colours; the roles that follow from an accent (its soft tint, the
//! selection, the focus ring and the verse numbers) are mixed from it below.

use std::sync::OnceLock;

use serde_json::Value;

pub use crate::records::{HighlightColor, Scheme, Theme};

const TOKENS: &str = include_str!("../../../apps/desktop/assets/design-tokens.json");

/// An sRGB colour with straight (not premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// `0xRRGGBBAA`.
    pub fn to_u32(self) -> u32 {
        u32::from_be_bytes([self.r, self.g, self.b, self.a])
    }

    fn hex(s: &str) -> Option<Self> {
        let h = s.strip_prefix('#')?;
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        let a = match h.len() {
            6 => 255,
            8 => byte(6)?,
            _ => return None,
        };
        Some(Self { r: byte(0)?, g: byte(2)?, b: byte(4)?, a })
    }

    /// Opaque, `t` of the way from `self` to `other`.
    fn mix(self, other: Rgba, t: f32) -> Self {
        let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
        Self::rgb(f(self.r, other.r), f(self.g, other.g), f(self.b, other.b))
    }

    fn alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }
}

/// One palette's colours. Field names are the design's colour roles (`color.<theme>.<role>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    /// Window and reading background.
    pub paper: Rgba,
    /// The sidebar, and grouped lists' background.
    pub sidebar: Rgba,
    /// Popovers, menus, cards, sheets.
    pub surface: Rgba,
    /// Toolbar material once content scrolls beneath it.
    pub toolbar: Rgba,
    /// Toolbar control capsules; the tint of glass.
    pub glass: Rgba,
    /// Text fields and selects.
    pub field: Rgba,
    /// A raised control: selected segment, toggle and slider knobs.
    pub raised: Rgba,
    /// Tracks, wells and hover fills.
    pub fill: Rgba,
    /// Pressed fills, the off track of a toggle.
    pub fill_strong: Rgba,
    /// Opaque hairlines and dividers.
    pub separator: Rgba,
    /// Translucent control outlines.
    pub border: Rgba,
    /// Modal backdrop.
    pub scrim: Rgba,
    /// Text: primary, secondary, tertiary (placeholders, disabled glyphs).
    pub ink: Rgba,
    pub ink_2: Rgba,
    pub ink_3: Rgba,
    /// Verse numbers.
    pub verse: Rgba,
    /// Red letters (words of Christ).
    pub rubric: Rgba,
    /// Selection, links and primary buttons; `on_accent` is text on it.
    pub accent: Rgba,
    pub on_accent: Rgba,
    /// Selected rows and chips.
    pub accent_soft: Rgba,
    /// Selected-verse wash, translucent so highlights show through.
    pub selection: Rgba,
    /// Keyboard focus ring.
    pub focus: Rgba,
    pub danger: Rgba,
    pub success: Rgba,
    /// Highlight fills behind text, in [`HighlightColor::ALL`] order.
    pub highlights: [Rgba; 6],
    /// Highlight identity colours (dots, filters); the same in every palette.
    pub swatches: [Rgba; 6],
}

impl Palette {
    pub fn highlight(&self, c: HighlightColor) -> Rgba {
        self.highlights[HighlightColor::ALL.iter().position(|x| *x == c).unwrap_or(0)]
    }

    pub fn swatch(&self, c: HighlightColor) -> Rgba {
        self.swatches[HighlightColor::ALL.iter().position(|x| *x == c).unwrap_or(0)]
    }
}

impl Scheme {
    pub const ALL: [Scheme; 3] = [Scheme::System, Scheme::Light, Scheme::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Scheme::System => "System",
            Scheme::Light => "Light",
            Scheme::Dark => "Dark",
        }
    }

    /// Whether the dark palette shows, given whether the system is dark.
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            Scheme::System => system_dark,
            Scheme::Light => false,
            Scheme::Dark => true,
        }
    }
}

impl Theme {
    pub const ALL: [Theme; 6] =
        [Theme::Linger, Theme::Paper, Theme::Forest, Theme::Oxblood, Theme::Plum, Theme::Graphite];

    pub fn label(self) -> &'static str {
        match self {
            Theme::Linger => "Linger",
            Theme::Paper => "Paper",
            Theme::Forest => "Forest",
            Theme::Oxblood => "Oxblood",
            Theme::Plum => "Plum",
            Theme::Graphite => "Graphite",
        }
    }

    /// The light or dark palette.
    pub fn palette(self, dark: bool) -> &'static Palette {
        &palettes()[self as usize * 2 + dark as usize]
    }

    /// The accents of the themes on Linger's neutrals, light and dark (the owner's choices).
    fn accents(self) -> Option<(Rgba, Rgba)> {
        match self {
            Theme::Forest => Some((Rgba::rgb(0x2F, 0x6B, 0x4F), Rgba::rgb(0x7C, 0xC4, 0xA0))),
            Theme::Oxblood => Some((Rgba::rgb(0x8E, 0x2B, 0x2B), Rgba::rgb(0xE8, 0x90, 0x8A))),
            Theme::Plum => Some((Rgba::rgb(0x6B, 0x3F, 0x7A), Rgba::rgb(0xC9, 0xA2, 0xDA))),
            Theme::Graphite => Some((Rgba::rgb(0x4A, 0x4D, 0x55), Rgba::rgb(0xB7, 0xBB, 0xC6))),
            Theme::Linger | Theme::Paper => None,
        }
    }
}

/// Paper's dark: its paper, ink and accent; the other roles are mixed from them.
const PAPER_DARK: (Rgba, Rgba, Rgba) =
    (Rgba::rgb(0x1C, 0x19, 0x15), Rgba::rgb(0xE8, 0xDF, 0xCF), Rgba::rgb(0xD9, 0xA8, 0x6C));

/// Every theme's light and dark palette, in [`Theme::ALL`] order, light first.
fn palettes() -> &'static [Palette] {
    static PALETTES: OnceLock<Vec<Palette>> = OnceLock::new();
    PALETTES.get_or_init(|| {
        let tokens: Value = serde_json::from_str(TOKENS).expect("design-tokens.json is JSON");
        let light = from_tokens(&tokens, "light");
        let dark = from_tokens(&tokens, "dark");
        let mut out = Vec::new();
        for theme in Theme::ALL {
            match theme {
                Theme::Linger => out.extend([light.clone(), dark.clone()]),
                Theme::Paper => {
                    let (paper, ink, accent) = PAPER_DARK;
                    out.extend([from_tokens(&tokens, "sepia"), warm_dark(&dark, paper, ink, accent)]);
                }
                _ => {
                    let (l, d) = theme.accents().expect("an accent theme");
                    out.extend([with_accent(&light, l, false), with_accent(&dark, d, true)]);
                }
            }
        }
        out
    })
}

/// `color.<theme>` and its highlight fills, as the token file has them.
fn from_tokens(tokens: &Value, theme: &str) -> Palette {
    let color = &tokens["color"];
    let c = &color[theme];
    let role = |r: &str| {
        c[r].as_str().and_then(Rgba::hex).unwrap_or_else(|| panic!("design-tokens.json: color.{theme}.{r}"))
    };
    let hl = |key: &str| {
        HighlightColor::ALL.map(|h| {
            color["highlight"][h.name()][key]
                .as_str()
                .and_then(Rgba::hex)
                .unwrap_or_else(|| panic!("design-tokens.json: color.highlight.{}.{key}", h.name()))
        })
    };
    Palette {
        paper: role("paper"),
        sidebar: role("sidebar"),
        surface: role("surface"),
        toolbar: role("toolbar"),
        glass: role("glass"),
        field: role("field"),
        raised: role("raised"),
        fill: role("fill"),
        fill_strong: role("fill_strong"),
        separator: role("separator"),
        border: role("border"),
        scrim: role("scrim"),
        ink: role("ink"),
        ink_2: role("ink_2"),
        ink_3: role("ink_3"),
        verse: role("verse"),
        rubric: role("rubric"),
        accent: role("accent"),
        on_accent: role("on_accent"),
        accent_soft: role("accent_soft"),
        selection: role("selection"),
        focus: role("focus"),
        danger: role("danger"),
        success: role("success"),
        highlights: hl(theme),
        swatches: hl("swatch"),
    }
}

/// `base` with another accent, and the roles that follow from it, mixed as the design mixes
/// Linger's own: in light the tint is 88 % toward the paper, the selection 14 % and the focus
/// ring 40 % of the accent; in dark the tint is 80 % toward the paper, the selection 20 % and the
/// focus 50 %, and text on the accent is nearly black. Verse numbers are the accent greyed toward
/// the tertiary ink.
fn with_accent(base: &Palette, accent: Rgba, dark: bool) -> Palette {
    let black = Rgba::rgb(0, 0, 0);
    Palette {
        accent,
        on_accent: if dark { black.mix(accent, 0.12) } else { base.on_accent },
        accent_soft: accent.mix(base.paper, if dark { 0.8 } else { 0.88 }),
        selection: accent.alpha(if dark { 0x33 } else { 0x24 }),
        focus: accent.alpha(if dark { 0x80 } else { 0x66 }),
        verse: accent.mix(base.ink_3, if dark { 0.35 } else { 0.6 }),
        ..base.clone()
    }
}

/// A dark palette from its paper, ink and accent: each neutral sits where Linger's dark puts it
/// between its paper and ink, and the status colours and highlights are Linger's dark ones.
fn warm_dark(linger: &Palette, paper: Rgba, ink: Rgba, accent: Rgba) -> Palette {
    let toward_ink = |t: f32| paper.mix(ink, t);
    let neutrals = Palette {
        paper,
        sidebar: toward_ink(0.058),
        surface: toward_ink(0.077),
        toolbar: paper.alpha(0xCC),
        glass: toward_ink(0.125).alpha(0xE0),
        field: toward_ink(0.038),
        raised: toward_ink(0.23),
        fill: ink.alpha(0x0F),
        fill_strong: ink.alpha(0x1C),
        separator: toward_ink(0.115),
        border: ink.alpha(0x1F),
        ink,
        ink_2: ink.mix(paper, 0.33),
        ink_3: ink.mix(paper, 0.6),
        ..linger.clone()
    };
    with_accent(&neutrals, accent, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linger_and_paper_light_are_the_tokens() {
        let light = Theme::Linger.palette(false);
        assert_eq!(light.paper, Rgba::rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(light.verse, Rgba::rgb(0x73, 0x83, 0xB0));
        assert_eq!(light.selection, Rgba { r: 0x2D, g: 0x4A, b: 0x9E, a: 0x24 });
        assert_eq!(Theme::Linger.palette(true).accent, Rgba::rgb(0x8E, 0xA6, 0xF2));
        assert_eq!(Theme::Paper.palette(false).paper, Rgba::rgb(0xF6, 0xEE, 0xDC));
        assert_eq!(
            Theme::Paper.palette(false).highlight(HighlightColor::Yellow),
            Rgba::rgb(0xEE, 0xD8, 0x96)
        );
    }

    #[test]
    fn accent_themes_keep_lingers_neutrals_and_take_their_accent() {
        let (linger, forest) = (Theme::Linger.palette(true), Theme::Forest.palette(true));
        assert_eq!(forest.accent, Rgba::rgb(0x7C, 0xC4, 0xA0));
        assert_eq!(
            (forest.paper, forest.ink, forest.highlights),
            (linger.paper, linger.ink, linger.highlights)
        );
        assert_eq!(forest.selection, Rgba { a: 0x33, ..forest.accent });
        // Verse numbers follow the accent: greener than they are blue.
        assert!(forest.verse.g > forest.verse.b && linger.verse.b > linger.verse.g);
        assert_eq!(Theme::Oxblood.palette(false).accent, Rgba::rgb(0x8E, 0x2B, 0x2B));
        assert_eq!(Theme::Oxblood.palette(false).on_accent, Rgba::rgb(0xFF, 0xFF, 0xFF));
    }

    #[test]
    fn paper_dark_is_warm_and_mixed_as_lingers_dark() {
        let p = Theme::Paper.palette(true);
        assert_eq!((p.paper, p.ink, p.accent), PAPER_DARK);
        // The mixing factors reproduce how light Linger's own dark neutrals are, from its paper
        // and ink (its greys are a touch cooler than its ink, so the hue isn't compared).
        let linger = Theme::Linger.palette(true);
        let made = warm_dark(linger, linger.paper, linger.ink, linger.accent);
        for (a, b) in [
            (made.sidebar, linger.sidebar),
            (made.surface, linger.surface),
            (made.raised, linger.raised),
            (made.separator, linger.separator),
            (made.ink_2, linger.ink_2),
            (made.ink_3, linger.ink_3),
        ] {
            let light = |c: Rgba| (c.r as i32 + c.g as i32 + c.b as i32) / 3;
            assert!((light(a) - light(b)).abs() <= 3, "{a:?} against {b:?}");
        }
    }

    #[test]
    fn the_scheme_picks_light_or_dark() {
        assert!(Scheme::System.is_dark(true) && !Scheme::System.is_dark(false));
        assert!(Scheme::Dark.is_dark(false) && !Scheme::Light.is_dark(true));
    }
}
