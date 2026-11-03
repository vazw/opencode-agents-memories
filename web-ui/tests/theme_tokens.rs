//! The palette is Rust data, and Thaw's theme is written from it.
//!
//! These run in node mode with no DOM: `palette` and `thaw_theme` are pure.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::theme::{Scale, Theme, palette, thaw_theme, token_css};

#[wasm_bindgen_test]
fn test_palette_each_theme_is_its_own() {
    let light = palette(Theme::Light);
    let dark = palette(Theme::Dark);
    assert_ne!(light, dark);
    assert_eq!(light.canvas, "#f7f8fa");
    assert_eq!(dark.canvas, "#14171c");
    assert_ne!(light.panel, dark.panel);
    assert_ne!(light.text, dark.text);
}

#[wasm_bindgen_test]
fn test_palette_values_are_the_tuned_screens() {
    let light = palette(Theme::Light);
    let got = [
        light.canvas,
        light.panel,
        light.border,
        light.text,
        light.muted,
        light.accent,
        light.accent_text,
        light.warn_bg,
        light.warn_border,
        light.warn_text,
    ];
    assert_eq!(
        got,
        [
            "#f7f8fa", "#ffffff", "#d9dce1", "#1b1f24", "#5a6472", "#2f6feb", "#ffffff", "#fff6e0",
            "#e0b44a", "#6b4a00",
        ]
    );
    let dark = palette(Theme::Dark);
    let got = [
        dark.canvas,
        dark.panel,
        dark.border,
        dark.text,
        dark.muted,
        dark.accent,
        dark.accent_text,
        dark.warn_bg,
        dark.warn_border,
        dark.warn_text,
    ];
    assert_eq!(
        got,
        [
            "#14171c", "#1c2027", "#2e3540", "#e8eaed", "#a0a8b4", "#7aa2f7", "#0d1117", "#2a2417",
            "#8a6d2a", "#f1d9a0",
        ]
    );
}

#[wasm_bindgen_test]
fn test_palette_fonts_are_one_stack_each() {
    let light = palette(Theme::Light);
    let dark = palette(Theme::Dark);
    assert_eq!(light.font_sans, dark.font_sans);
    assert_eq!(light.font_mono, dark.font_mono);
    assert_ne!(light.font_sans, light.font_mono);
    assert!(light.font_sans.starts_with("system-ui"));
    assert!(light.font_mono.starts_with("ui-monospace"));
}

/// `(field, mapped value in the Thaw theme, the palette value it must equal)`.
fn mapped(theme: Theme) -> Vec<(&'static str, String, &'static str)> {
    let p = palette(theme);
    let c = thaw_theme(theme).color;
    vec![
        (
            "color_neutral_background_1",
            c.color_neutral_background_1,
            p.panel,
        ),
        (
            "color_neutral_background_3",
            c.color_neutral_background_3,
            p.canvas,
        ),
        (
            "color_neutral_background_4",
            c.color_neutral_background_4,
            p.panel,
        ),
        (
            "color_neutral_background_4_hover",
            c.color_neutral_background_4_hover,
            p.surface_hover,
        ),
        (
            "color_neutral_background_4_pressed",
            c.color_neutral_background_4_pressed,
            p.surface_active,
        ),
        (
            "color_neutral_background_1_hover",
            c.color_neutral_background_1_hover,
            p.surface_hover,
        ),
        (
            "color_neutral_background_1_pressed",
            c.color_neutral_background_1_pressed,
            p.surface_active,
        ),
        (
            "color_compound_brand_stroke",
            c.color_compound_brand_stroke,
            p.accent,
        ),
        (
            "color_compound_brand_stroke_pressed",
            c.color_compound_brand_stroke_pressed,
            p.accent_pressed,
        ),
        ("color_neutral_stroke_1", c.color_neutral_stroke_1, p.border),
        (
            "color_neutral_stroke_1_hover",
            c.color_neutral_stroke_1_hover,
            p.accent,
        ),
        ("color_stroke_focus_2", c.color_stroke_focus_2, p.accent),
        (
            "color_palette_yellow_background_1",
            c.color_palette_yellow_background_1,
            p.warn_bg,
        ),
        (
            "color_palette_yellow_border_1",
            c.color_palette_yellow_border_1,
            p.warn_border,
        ),
        (
            "color_neutral_stroke_1_pressed",
            c.color_neutral_stroke_1_pressed,
            p.border,
        ),
        ("color_neutral_stroke_2", c.color_neutral_stroke_2, p.border),
        (
            "color_neutral_foreground_1",
            c.color_neutral_foreground_1,
            p.text,
        ),
        (
            "color_neutral_foreground_2",
            c.color_neutral_foreground_2,
            p.text,
        ),
        (
            "color_neutral_foreground_3",
            c.color_neutral_foreground_3,
            p.muted,
        ),
        (
            "color_neutral_foreground_2_brand_selected",
            c.color_neutral_foreground_2_brand_selected,
            p.accent,
        ),
        (
            "color_neutral_foreground_on_brand",
            c.color_neutral_foreground_on_brand,
            p.accent_text,
        ),
        ("color_brand_background", c.color_brand_background, p.accent),
        (
            "color_brand_background_hover",
            c.color_brand_background_hover,
            p.accent,
        ),
        (
            "color_brand_background_pressed",
            c.color_brand_background_pressed,
            p.accent,
        ),
        ("color_brand_stroke_1", c.color_brand_stroke_1, p.accent),
        (
            "color_brand_foreground_1",
            c.color_brand_foreground_1,
            p.accent,
        ),
        (
            "color_brand_foreground_2",
            c.color_brand_foreground_2,
            p.accent,
        ),
        (
            "color_brand_foreground_link",
            c.color_brand_foreground_link,
            p.accent,
        ),
        (
            "color_palette_yellow_foreground_1",
            c.color_palette_yellow_foreground_1,
            p.warn_text,
        ),
    ]
}

#[wasm_bindgen_test]
fn test_thaw_theme_colour_tokens_come_from_the_palette() {
    // Both themes: `color_neutral_background_1` is vacuous in the light pass
    // (panel `#ffffff` is also Thaw's stock light value) and only the dark
    // pass separates mapped from unmapped.
    for theme in [Theme::Light, Theme::Dark] {
        let rows = mapped(theme);
        assert_eq!(rows.len(), 29);
        for (field, got, want) in rows {
            assert_eq!(got, want, "{field} under {theme:?}");
        }
    }
}

#[wasm_bindgen_test]
fn test_thaw_theme_uses_the_palette_typography() {
    for theme in [Theme::Light, Theme::Dark] {
        let p = palette(theme);
        let common = thaw_theme(theme).common;
        assert_eq!(common.font_family_base, p.font_sans);
        assert_eq!(common.font_family_numeric, p.font_sans);
        assert_eq!(common.font_family_monospace, p.font_mono);
    }
}

#[wasm_bindgen_test]
fn test_thaw_theme_keeps_the_tuned_base_size() {
    for theme in [Theme::Light, Theme::Dark] {
        let common = thaw_theme(theme).common;
        assert_eq!(common.font_size_base_300, "14px");
        assert_eq!(common.line_height_base_300, "20px");
    }
}

#[wasm_bindgen_test]
fn test_thaw_theme_names_its_scheme() {
    let dark = thaw_theme(Theme::Dark);
    assert_eq!(dark.name, "dark");
    assert_eq!(dark.color.color_scheme, "dark");
    let light = thaw_theme(Theme::Light);
    assert_eq!(light.name, "light");
    assert_eq!(light.color.color_scheme, "light");
}

#[wasm_bindgen_test]
fn test_token_css_emits_each_app_var_once_and_no_colour() {
    let css = token_css(&Scale::default());
    for name in ["--app-measure", "--app-sider-w", "--app-header-h"] {
        assert_eq!(css.matches(name).count(), 1, "{name} in {css}");
    }
    assert_eq!(css.matches("--app-").count(), 3);
    assert!(!css.contains('#'), "tokens carry no colour: {css}");
    assert!(css.contains("--app-measure: 72ch"));
    assert!(css.contains("--app-sider-w: 260px"));
    assert!(css.contains("--app-header-h: 56px"));
}

#[wasm_bindgen_test]
fn test_new_palette_ramps_are_distinct_per_theme() {
    for theme in [Theme::Light, Theme::Dark] {
        let p = palette(theme);
        assert_ne!(
            p.surface_hover, p.canvas,
            "hover must leave the canvas colour"
        );
        assert_ne!(p.surface_hover, p.surface_active);
        assert_ne!(p.accent_hover, p.accent);
        assert_ne!(p.accent_pressed, p.accent);
        assert_ne!(p.accent_soft, p.accent);
    }
}
