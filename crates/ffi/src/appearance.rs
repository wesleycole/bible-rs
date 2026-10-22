//! Appearance: the themes' palettes, as `linger_core::appearance` defines them for both apps.
//! Colours cross as `0xRRGGBBAA`, alpha not premultiplied.

use linger_core::appearance as core;

/// System, Light or Dark: which of a theme's two palettes shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AppearanceScheme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ThemeName {
    Linger,
    Paper,
    Forest,
    Oxblood,
    Plum,
    Graphite,
}

macro_rules! same_enum {
    ($ours:ident, $theirs:ty, $($v:ident),+) => {
        impl From<$theirs> for $ours {
            fn from(x: $theirs) -> Self {
                match x { $(<$theirs>::$v => Self::$v),+ }
            }
        }
        impl From<$ours> for $theirs {
            fn from(x: $ours) -> Self {
                match x { $($ours::$v => Self::$v),+ }
            }
        }
    };
}

same_enum!(AppearanceScheme, core::Scheme, System, Light, Dark);
same_enum!(ThemeName, core::Theme, Linger, Paper, Forest, Oxblood, Plum, Graphite);

/// One palette. Field names are the design's colour roles; see `linger_core::appearance::Palette`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ThemePalette {
    pub paper: u32,
    pub sidebar: u32,
    pub surface: u32,
    pub toolbar: u32,
    pub glass: u32,
    pub field: u32,
    pub raised: u32,
    pub fill: u32,
    pub fill_strong: u32,
    pub separator: u32,
    pub border: u32,
    pub scrim: u32,
    pub ink: u32,
    pub ink_2: u32,
    pub ink_3: u32,
    pub verse: u32,
    pub rubric: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub accent_soft: u32,
    pub selection: u32,
    pub focus: u32,
    pub danger: u32,
    pub success: u32,
    /// Highlight fills behind text, in `highlight_colors()` order.
    pub highlights: Vec<u32>,
    /// Highlight identity colours (the verse row's dots), in the same order.
    pub swatches: Vec<u32>,
}

/// The six themes, in the order Settings › Appearance shows them.
#[uniffi::export]
pub fn themes() -> Vec<ThemeName> {
    core::Theme::ALL.into_iter().map(Into::into).collect()
}

/// A theme's name as shown.
#[uniffi::export]
pub fn theme_label(theme: ThemeName) -> String {
    core::Theme::from(theme).label().into()
}

/// A theme's light or dark palette.
#[uniffi::export]
pub fn theme_palette(theme: ThemeName, dark: bool) -> ThemePalette {
    let p = core::Theme::from(theme).palette(dark);
    let c = |x: core::Rgba| x.to_u32();
    ThemePalette {
        paper: c(p.paper),
        sidebar: c(p.sidebar),
        surface: c(p.surface),
        toolbar: c(p.toolbar),
        glass: c(p.glass),
        field: c(p.field),
        raised: c(p.raised),
        fill: c(p.fill),
        fill_strong: c(p.fill_strong),
        separator: c(p.separator),
        border: c(p.border),
        scrim: c(p.scrim),
        ink: c(p.ink),
        ink_2: c(p.ink_2),
        ink_3: c(p.ink_3),
        verse: c(p.verse),
        rubric: c(p.rubric),
        accent: c(p.accent),
        on_accent: c(p.on_accent),
        accent_soft: c(p.accent_soft),
        selection: c(p.selection),
        focus: c(p.focus),
        danger: c(p.danger),
        success: c(p.success),
        highlights: p.highlights.iter().map(|x| x.to_u32()).collect(),
        swatches: p.swatches.iter().map(|x| x.to_u32()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_palette_crosses_as_rrggbbaa() {
        let p = theme_palette(ThemeName::Linger, false);
        assert_eq!((p.paper, p.accent, p.selection), (0xFFFFFFFF, 0x2D4A9EFF, 0x2D4A9E24));
        assert_eq!(theme_palette(ThemeName::Forest, true).accent, 0x7CC4A0FF);
        assert_eq!(p.highlights.len(), 6);
    }
}
