//! The baseline theme.
//!
//! Two themes only (light / dark), upstream parity: the default is dark and the
//! preference lives under `opencode-mem-theme` in `localStorage`. That key is a
//! UI preference, never a credential.
//!
//! The read/write logic is a pure function over the `ThemeStore` seam so it can
//! be asserted in node mode; `LocalStorageThemeStore` is the thin browser
//! adapter.

/// The theme preference key (upstream parity).
pub const THEME_STORAGE_KEY: &str = "opencode-mem-theme";

/// The two themes this shell ships.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Theme {
    Light,
    #[default]
    Dark,
}

impl Theme {
    /// The value persisted in the theme key. Only `light`/`dark` are ever
    /// written; anything else read back falls back to the default.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// The one place a colour or a font stack is written. `thaw_theme` copies it
/// into Thaw's tokens and `apply_canvas` paints the page from it; the
/// stylesheet holds only `var(--…)` aliases of those tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub canvas: &'static str,
    pub panel: &'static str,
    pub border: &'static str,
    pub text: &'static str,
    pub muted: &'static str,
    pub accent: &'static str,
    pub accent_text: &'static str,
    pub accent_hover: &'static str,
    pub accent_pressed: &'static str,
    pub accent_soft: &'static str,
    pub surface_hover: &'static str,
    pub surface_active: &'static str,
    pub warn_bg: &'static str,
    pub warn_border: &'static str,
    pub warn_text: &'static str,
    pub font_sans: &'static str,
    pub font_mono: &'static str,
}

const FONT_SANS: &str =
    "system-ui, -apple-system, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, sans-serif";
const FONT_MONO: &str = "ui-monospace, SFMono-Regular, \"SF Mono\", Menlo, monospace";

/// The palette of a theme (fonts are the same for both).
pub fn palette(theme: Theme) -> Palette {
    match theme {
        Theme::Light => Palette {
            canvas: "#f7f8fa",
            panel: "#ffffff",
            border: "#d9dce1",
            text: "#1b1f24",
            muted: "#5a6472",
            accent: "#2f6feb",
            accent_text: "#ffffff",
            accent_hover: "#4a82f0",
            accent_pressed: "#2559c4",
            accent_soft: "#e3ecfd",
            surface_hover: "#eef0f3",
            surface_active: "#e3e6ea",
            warn_bg: "#fff6e0",
            warn_border: "#e0b44a",
            warn_text: "#6b4a00",
            font_sans: FONT_SANS,
            font_mono: FONT_MONO,
        },
        Theme::Dark => Palette {
            canvas: "#14171c",
            panel: "#1c2027",
            border: "#2e3540",
            text: "#e8eaed",
            muted: "#a0a8b4",
            accent: "#7aa2f7",
            accent_text: "#0d1117",
            accent_hover: "#8fb1f9",
            accent_pressed: "#6690e8",
            accent_soft: "#1f2a40",
            surface_hover: "#232830",
            surface_active: "#2a303a",
            warn_bg: "#2a2417",
            warn_border: "#8a6d2a",
            warn_text: "#f1d9a0",
            font_sans: FONT_SANS,
            font_mono: FONT_MONO,
        },
    }
}

/// Thaw's theme, written from the palette. Starts from Thaw's own light/dark
/// (so every field not mapped keeps a valid value) and overwrites the 25 colour
/// tokens and the three font families. The base size (14px/20px) is left alone:
/// it is what the app already renders.
pub fn thaw_theme(theme: Theme) -> thaw::Theme {
    let p = palette(theme);
    let mut out = match theme {
        Theme::Light => thaw::Theme::light(),
        Theme::Dark => thaw::Theme::dark(),
    };
    let c = &mut out.color;
    let set = |slot: &mut String, value: &str| *slot = value.to_string();
    set(&mut c.color_neutral_background_1, p.panel);
    set(&mut c.color_neutral_background_3, p.canvas);
    set(&mut c.color_neutral_background_4, p.panel);
    set(&mut c.color_neutral_background_1_hover, p.surface_hover);
    set(&mut c.color_neutral_background_1_pressed, p.surface_active);
    set(&mut c.color_neutral_background_4_hover, p.surface_hover);
    set(&mut c.color_neutral_background_4_pressed, p.surface_active);
    set(&mut c.color_compound_brand_stroke, p.accent);
    set(&mut c.color_compound_brand_stroke_pressed, p.accent_pressed);
    set(&mut c.color_neutral_stroke_1, p.border);
    set(&mut c.color_neutral_stroke_1_hover, p.accent);
    set(&mut c.color_stroke_focus_2, p.accent);
    set(&mut c.color_palette_yellow_background_1, p.warn_bg);
    set(&mut c.color_palette_yellow_border_1, p.warn_border);
    set(&mut c.color_neutral_stroke_1_pressed, p.border);
    set(&mut c.color_neutral_stroke_2, p.border);
    set(&mut c.color_neutral_foreground_1, p.text);
    set(&mut c.color_neutral_foreground_2, p.text);
    set(&mut c.color_neutral_foreground_3, p.muted);
    set(&mut c.color_neutral_foreground_2_brand_selected, p.accent);
    set(&mut c.color_neutral_foreground_on_brand, p.accent_text);
    set(&mut c.color_brand_background, p.accent);
    set(&mut c.color_brand_background_hover, p.accent);
    set(&mut c.color_brand_background_pressed, p.accent);
    set(&mut c.color_brand_stroke_1, p.accent);
    set(&mut c.color_brand_foreground_1, p.accent);
    set(&mut c.color_brand_foreground_2, p.accent);
    set(&mut c.color_brand_foreground_link, p.accent);
    set(&mut c.color_palette_yellow_foreground_1, p.warn_text);
    out.common.font_family_base = p.font_sans.to_string();
    out.common.font_family_numeric = p.font_sans.to_string();
    out.common.font_family_monospace = p.font_mono.to_string();
    out
}

/// The theme toggle's glyph: the sun while light, the moon while dark.
pub fn toggle_icon(theme: Theme) -> icondata_core::Icon {
    match theme {
        Theme::Light => icondata_mdi::MdiWeatherSunny,
        Theme::Dark => icondata_mdi::MdiWeatherNight,
    }
}

/// The layout scale: the only three measures with a stylesheet consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scale {
    /// Reading measure (`.memory-content`).
    pub measure: &'static str,
    /// Text sider width (`.app-sider`).
    pub sider_w: &'static str,
    /// Sticky header height (`.app-header`, the sticky filter bar offset).
    pub header_h: &'static str,
}

impl Default for Scale {
    fn default() -> Self {
        Self {
            measure: "72ch",
            sider_w: "260px",
            header_h: "56px",
        }
    }
}

/// The CSS for the scale tokens: exactly three `--app-*` names, no colour, no
/// user-controlled string.
pub fn token_css(scale: &Scale) -> String {
    format!(
        ":root {{ --app-measure: {}; --app-sider-w: {}; --app-header-h: {}; }}",
        scale.measure, scale.sider_w, scale.header_h
    )
}

/// The id of the one token `<style>` element.
pub const TOKEN_STYLE_ID: &str = "app-tokens";

/// Insert the `<style id="app-tokens">` once; a no-op when it already exists
/// or there is no document.
pub fn install_tokens(scale: &Scale) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if doc.get_element_by_id(TOKEN_STYLE_ID).is_some() {
        return;
    }
    let Ok(style) = doc.create_element("style") else {
        return;
    };
    let _ = style.set_attribute("id", TOKEN_STYLE_ID);
    style.set_text_content(Some(&token_css(scale)));
    if let Some(head) = doc.head() {
        let _ = head.append_child(&style);
    }
}

/// Paint the document canvas from the palette. Thaw scopes its variables to the
/// provider `div`, which `html` cannot read, so the root background is written
/// directly; otherwise a short page shows unpainted canvas under the shell.
/// A missing window/element is a no-op (the paint is cosmetic).
pub fn apply_canvas(p: &Palette) {
    use wasm_bindgen::JsCast;
    let root = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok());
    if let Some(root) = root {
        let _ = root.style().set_property("background-color", p.canvas);
    }
}

/// The persistence seam: `localStorage` in the browser, an in-memory fake in
/// the tests. Named `ThemeStore` to keep `web_sys::Storage` unambiguous.
pub trait ThemeStore {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str);
}

/// Read the preference, defaulting to dark when nothing valid is stored.
pub fn read_preference(store: &dyn ThemeStore) -> Theme {
    match store.get(THEME_STORAGE_KEY).as_deref() {
        Some("light") => Theme::Light,
        Some("dark") => Theme::Dark,
        _ => Theme::default(),
    }
}

/// Persist the preference under the theme key.
pub fn store_preference(store: &dyn ThemeStore, theme: Theme) {
    store.set(THEME_STORAGE_KEY, theme.as_str());
}

/// The browser adapter over `window.localStorage`.
///
/// A missing window or a storage failure (private mode, disabled storage) reads
/// as "no preference" and a write failure is ignored: the theme is a preference,
/// not state the shell depends on.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalStorageThemeStore;

impl ThemeStore for LocalStorageThemeStore {
    fn get(&self, key: &str) -> Option<String> {
        let storage = web_sys::window()?.local_storage().ok()??;
        storage.get_item(key).ok()?
    }

    fn set(&self, key: &str, value: &str) {
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(key, value);
        }
    }
}
