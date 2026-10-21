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

impl From<Theme> for thaw::Theme {
    fn from(theme: Theme) -> Self {
        match theme {
            Theme::Light => thaw::Theme::light(),
            Theme::Dark => thaw::Theme::dark(),
        }
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
