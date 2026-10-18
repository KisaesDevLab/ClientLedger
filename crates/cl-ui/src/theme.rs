//! Theme tokens from docs/DESIGN.md, light and dark, and how they are applied
//! to egui.
//!
//! Every colour in the app comes from [`Tokens`]; screens call
//! [`Tokens::of`] with `ui.ctx()` so a theme switch repaints everything.
//! Dark values for roles DESIGN.md lists only for light are derived with the
//! same role and tuned to keep text at 4.5:1 (see the contrast tests).

use eframe::egui::{
    self, Color32, Context, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Theme,
    ThemePreference, Vec2, Visuals,
};

pub use cl_settings::ThemeChoice;

/// egui's view of a theme choice.
fn preference(choice: ThemeChoice) -> ThemePreference {
    match choice {
        ThemeChoice::Light => ThemePreference::Light,
        ThemeChoice::Dark => ThemePreference::Dark,
        ThemeChoice::Auto => ThemePreference::System,
    }
}

/// Every colour token in DESIGN.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)] // names are the DESIGN.md token names
pub struct Tokens {
    pub chrome_title: Color32,
    pub chrome_title_text: Color32,
    pub chrome_title_muted: Color32,
    pub surface_app: Color32,
    pub surface_panel: Color32,
    pub surface_window: Color32,
    pub surface_subtle: Color32,
    pub surface_form: Color32,
    pub border_strong: Color32,
    pub border_default: Color32,
    pub border_row: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
    pub text_placeholder: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_tint: Color32,
    pub accent_border: Color32,
    pub warn: Color32,
    /// Fill behind count badges: `warn`, darkened in the light theme so white
    /// text on it reaches 4.5:1 (Q-P06c).
    pub warn_badge: Color32,
    pub warn_tint: Color32,
    pub warn_surface: Color32,
    pub warn_text: Color32,
    pub info: Color32,
    pub info_tint: Color32,
    pub neutral_chip: Color32,
    pub neutral_chip_text: Color32,
    pub locked_row: Color32,
    pub locked_text: Color32,
    /// Text on an `accent` fill (primary buttons, the active rail item).
    pub on_accent: Color32,
    /// Text on a `warn` fill (count badges).
    pub on_warn: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    // Truncation is the point: pick the three bytes.
    #[allow(clippy::cast_possible_truncation)]
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

impl Tokens {
    /// Light tokens, exactly as in DESIGN.md.
    pub const LIGHT: Tokens = Tokens {
        chrome_title: hex(0x1E_2421),
        chrome_title_text: hex(0xE9_E6E0),
        chrome_title_muted: hex(0xA9_B0AB),
        surface_app: hex(0xE9_E6E0),
        surface_panel: hex(0xF7_F5F1),
        surface_window: hex(0xFF_FFFF),
        surface_subtle: hex(0xFB_FAF8),
        surface_form: hex(0xF2_F6F5),
        border_strong: hex(0xC9_C4BA),
        border_default: hex(0xDA_D6CE),
        border_row: hex(0xEE_EBE5),
        text_primary: hex(0x1E_2421),
        text_secondary: hex(0x4F_5752),
        text_tertiary: hex(0x6B_726D),
        text_placeholder: hex(0x8C_928E),
        accent: hex(0x0F_5E5A),
        accent_hover: hex(0x0A_4441),
        accent_tint: hex(0xE6_F0EF),
        accent_border: hex(0xB9_C9C6),
        warn: hex(0xB5_651D),
        warn_badge: hex(0xA8_5C17),
        warn_tint: hex(0xFF_F8EF),
        warn_surface: hex(0xFF_F4E5),
        warn_text: hex(0x6B_3A0C),
        info: hex(0x23_406E),
        info_tint: hex(0xE8_EEF8),
        neutral_chip: hex(0xEE_EBE5),
        neutral_chip_text: hex(0x3A_403C),
        locked_row: hex(0xF6_F5F2),
        locked_text: hex(0x5C_625E),
        on_accent: hex(0xFF_FFFF),
        on_warn: hex(0xFF_FFFF),
    };

    /// Dark tokens: DESIGN.md starting values, plus derived roles.
    pub const DARK: Tokens = Tokens {
        chrome_title: hex(0x0C_0F0D),
        chrome_title_text: hex(0xE9_E6E0),
        chrome_title_muted: hex(0xA9_B0AB),
        surface_app: hex(0x12_1513),
        surface_panel: hex(0x1A_1E1C),
        surface_window: hex(0x20_2523),
        surface_subtle: hex(0x26_2B29),
        surface_form: hex(0x1F_2B29),
        border_strong: hex(0x3D_4541),
        border_default: hex(0x34_3B38),
        border_row: hex(0x2B_312E),
        text_primary: hex(0xE6_E9E7),
        text_secondary: hex(0xB4_BBB7),
        text_tertiary: hex(0x94_9C97),
        text_placeholder: hex(0x7D_857F),
        accent: hex(0x3F_A39C),
        accent_hover: hex(0x5B_B8B1),
        accent_tint: hex(0x1C_3533),
        accent_border: hex(0x2F_5A56),
        warn: hex(0xD9_8A45),
        warn_badge: hex(0xD9_8A45),
        warn_tint: hex(0x2E_2418),
        warn_surface: hex(0x3A_2C1A),
        warn_text: hex(0xF0_C48F),
        info: hex(0x8F_B0E8),
        info_tint: hex(0x1D_2638),
        neutral_chip: hex(0x2B_312E),
        neutral_chip_text: hex(0xD2_D8D4),
        locked_row: hex(0x1D_2220),
        locked_text: hex(0x98_A09B),
        on_accent: hex(0x0B_1110),
        on_warn: hex(0x1A_1208),
    };

    /// Tokens for a theme.
    #[must_use]
    pub fn for_theme(theme: Theme) -> Tokens {
        match theme {
            Theme::Light => Tokens::LIGHT,
            Theme::Dark => Tokens::DARK,
        }
    }

    /// Tokens for whatever theme `ctx` is showing right now.
    #[must_use]
    pub fn of(ctx: &Context) -> Tokens {
        Tokens::for_theme(ctx.theme())
    }

    /// Light or dark, from the tokens themselves.
    #[must_use]
    pub fn is_dark(&self) -> bool {
        self.surface_window == Tokens::DARK.surface_window
    }
}

/// Text style names beyond egui's built-ins (sizes from DESIGN.md typography).
pub mod styles {
    use eframe::egui::TextStyle;

    /// Section heading, 14/600.
    #[must_use]
    pub fn section() -> TextStyle {
        TextStyle::Name("section".into())
    }
    /// Eyebrow label, 11-13/600 uppercase.
    #[must_use]
    pub fn eyebrow() -> TextStyle {
        TextStyle::Name("eyebrow".into())
    }
    /// Form input text, 14.
    #[must_use]
    pub fn input() -> TextStyle {
        TextStyle::Name("input".into())
    }
    /// Keyboard chip text, 11 mono.
    #[must_use]
    pub fn kbd() -> TextStyle {
        TextStyle::Name("kbd".into())
    }
}

/// Install fonts (once) and both styles, and choose the theme preference.
pub fn apply(ctx: &Context, choice: ThemeChoice) {
    ctx.set_theme(preference(choice));
    ctx.set_style_of(Theme::Light, style_for(&Tokens::LIGHT, Theme::Light));
    ctx.set_style_of(Theme::Dark, style_for(&Tokens::DARK, Theme::Dark));
}

fn style_for(t: &Tokens, theme: Theme) -> egui::Style {
    let mut style = egui::Style::default();
    let mut v = match theme {
        Theme::Light => Visuals::light(),
        Theme::Dark => Visuals::dark(),
    };
    v.panel_fill = t.surface_panel;
    v.window_fill = t.surface_window;
    v.extreme_bg_color = t.surface_window; // text inputs
    v.faint_bg_color = t.surface_subtle;
    v.code_bg_color = t.neutral_chip;
    v.window_stroke = Stroke::new(1.0, t.border_strong);
    v.hyperlink_color = t.accent;
    v.warn_fg_color = t.warn;
    v.error_fg_color = if t.is_dark() {
        hex(0xF0_8A80)
    } else {
        hex(0xA3_2A22)
    };
    v.selection.bg_fill = t.accent_tint;
    v.selection.stroke = Stroke::new(2.0, t.accent);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(6);

    let radius = CornerRadius::same(4);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.surface_panel;
    w.noninteractive.weak_bg_fill = t.surface_panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.border_default);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text_primary);
    w.noninteractive.corner_radius = radius;
    w.inactive.bg_fill = t.surface_window;
    w.inactive.weak_bg_fill = t.surface_window;
    w.inactive.bg_stroke = Stroke::new(1.0, t.border_strong);
    w.inactive.fg_stroke = Stroke::new(1.0, t.text_primary);
    w.inactive.corner_radius = radius;
    w.hovered.bg_fill = t.accent_tint;
    w.hovered.weak_bg_fill = t.accent_tint;
    w.hovered.bg_stroke = Stroke::new(1.0, t.accent_border);
    w.hovered.fg_stroke = Stroke::new(1.5, t.text_primary);
    w.hovered.corner_radius = radius;
    w.active.bg_fill = t.accent_tint;
    w.active.weak_bg_fill = t.accent_tint;
    w.active.bg_stroke = Stroke::new(2.0, t.accent);
    w.active.fg_stroke = Stroke::new(2.0, t.text_primary);
    w.active.corner_radius = radius;
    w.open.bg_fill = t.accent_tint;
    w.open.weak_bg_fill = t.accent_tint;
    w.open.bg_stroke = Stroke::new(1.0, t.accent_border);
    w.open.fg_stroke = Stroke::new(1.0, t.text_primary);
    w.open.corner_radius = radius;
    style.visuals = v;

    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.window_margin = Margin::same(12);
    style.spacing.interact_size = Vec2::new(40.0, 28.0);
    style.spacing.menu_margin = Margin::same(6);
    style.interaction.tooltip_delay = 0.4;

    let sans = FontFamily::Proportional;
    let mono = FontFamily::Monospace;
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(22.0, crate::fonts::semibold()),
        ),
        (TextStyle::Body, FontId::new(13.0, sans.clone())),
        (TextStyle::Button, FontId::new(13.0, crate::fonts::medium())),
        (TextStyle::Small, FontId::new(12.0, sans.clone())),
        (TextStyle::Monospace, FontId::new(13.0, mono.clone())),
        (
            styles::section(),
            FontId::new(14.0, crate::fonts::semibold()),
        ),
        (
            styles::eyebrow(),
            FontId::new(11.5, crate::fonts::semibold()),
        ),
        (styles::input(), FontId::new(14.0, sans)),
        (styles::kbd(), FontId::new(11.0, mono)),
    ]
    .into();
    style
}

/// WCAG relative luminance of an opaque colour.
#[must_use]
pub fn luminance(c: Color32) -> f32 {
    fn channel(v: u8) -> f32 {
        let s = f32::from(v) / 255.0;
        if s <= 0.039_28 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// WCAG contrast ratio between two opaque colours (1.0 to 21.0).
#[must_use]
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AA: f32 = 4.5;

    fn pairs(t: &Tokens) -> Vec<(&'static str, Color32, Color32)> {
        // `warn` itself is a fill and mark colour (borders, dots); text on
        // warning surfaces uses `warn_text`.
        vec![
            ("primary on window", t.text_primary, t.surface_window),
            ("primary on panel", t.text_primary, t.surface_panel),
            ("primary on app", t.text_primary, t.surface_app),
            ("primary on subtle", t.text_primary, t.surface_subtle),
            ("primary on form", t.text_primary, t.surface_form),
            ("primary on accent tint", t.text_primary, t.accent_tint),
            ("primary on warn tint", t.text_primary, t.warn_tint),
            ("secondary on window", t.text_secondary, t.surface_window),
            ("secondary on panel", t.text_secondary, t.surface_panel),
            ("secondary on subtle", t.text_secondary, t.surface_subtle),
            ("tertiary on window", t.text_tertiary, t.surface_window),
            ("tertiary on panel", t.text_tertiary, t.surface_panel),
            ("accent link on window", t.accent, t.surface_window),
            ("accent link on panel", t.accent, t.surface_panel),
            ("on_accent on accent", t.on_accent, t.accent),
            ("on_warn on warn badge", t.on_warn, t.warn_badge),
            ("title text on title", t.chrome_title_text, t.chrome_title),
            ("title muted on title", t.chrome_title_muted, t.chrome_title),
            ("warn text on warn surface", t.warn_text, t.warn_surface),
            ("info on info tint", t.info, t.info_tint),
            ("chip text on chip", t.neutral_chip_text, t.neutral_chip),
            ("locked text on locked row", t.locked_text, t.locked_row),
        ]
    }

    #[test]
    fn every_text_pair_meets_4_5_to_1_in_both_themes() {
        let mut failures = Vec::new();
        for (name, tokens) in [("light", Tokens::LIGHT), ("dark", Tokens::DARK)] {
            for (pair, fg, bg) in pairs(&tokens) {
                let c = contrast(fg, bg);
                if c < AA {
                    failures.push(format!("{name}: {pair} = {c:.2}"));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn placeholders_are_hints_not_text_and_keep_3_to_1() {
        // DESIGN.md gives #8C928E for placeholders; that is a hint colour
        // (WCAG exempts placeholder-like hints only down to 3:1). Q-P06c.
        for t in [Tokens::LIGHT, Tokens::DARK] {
            assert!(contrast(t.text_placeholder, t.surface_window) >= 3.0);
        }
    }

    #[test]
    fn contrast_math_matches_known_values() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.001);
    }

    #[test]
    fn light_tokens_are_the_design_md_values() {
        let t = Tokens::LIGHT;
        assert_eq!(t.accent, Color32::from_rgb(0x0F, 0x5E, 0x5A));
        assert_eq!(t.surface_panel, Color32::from_rgb(0xF7, 0xF5, 0xF1));
        assert_eq!(t.chrome_title, Color32::from_rgb(0x1E, 0x24, 0x21));
        assert_eq!(
            Tokens::DARK.surface_window,
            Color32::from_rgb(0x20, 0x25, 0x23)
        );
        assert!(Tokens::DARK.is_dark() && !Tokens::LIGHT.is_dark());
    }

    #[test]
    fn theme_choice_cycles() {
        assert_eq!(ThemeChoice::Light.next(), ThemeChoice::Dark);
        assert_eq!(ThemeChoice::Dark.next(), ThemeChoice::Auto);
        assert_eq!(ThemeChoice::Auto.next(), ThemeChoice::Light);
    }
}
