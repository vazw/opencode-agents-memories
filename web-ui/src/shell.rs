//! The shell view: `ConfigProvider` + `Router` + the layout.
//!
//! `use_location()` is called by `ShellView`, which is rendered *inside* the
//! `<Router>` subtree. Calling it in a parent of the router panics at mount and
//! leaves an empty `<body>`.

use leptos::prelude::*;
use leptos_meta::{Title, provide_meta_context};
use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::path;
use thaw::ConfigProvider;

use crate::nav::{ScreenState, nav_entries, state_for};
use crate::routes::{resolve, route as route_spec};
use crate::screens::ScreenBody;
use crate::theme::{LocalStorageThemeStore, Theme, read_preference, store_preference};

/// The application shell.
#[component]
pub fn Shell() -> impl IntoView {
    provide_meta_context();

    let theme = RwSignal::new(read_preference(&LocalStorageThemeStore));
    provide_context(theme);

    // thaw's theme is a separate signal, kept in sync with the preference (D6).
    let thaw_theme: RwSignal<thaw::Theme> = RwSignal::new(theme.get().into());
    Effect::new(move |_| {
        let current = theme.get();
        thaw_theme.set(current.into());
        store_preference(&LocalStorageThemeStore, current);
    });

    view! {
        <ConfigProvider theme=thaw_theme>
            <Router>
                <Routes fallback=|| ()>
                    <Route path=path!("/*any") view=ShellView/>
                </Routes>
            </Router>
        </ConfigProvider>
    }
}

/// The routed body. This is where `use_location()` is legal.
#[component]
fn ShellView() -> impl IntoView {
    let location = leptos_router::hooks::use_location();
    let theme = expect_context::<RwSignal<Theme>>();
    let current_route = Memo::new(move |_| resolve(&location.pathname.get()));

    view! {
        <div class="app-shell" class:dark=move || theme.get() == Theme::Dark>
            <header class="app-header">
                <h1 class="app-title">"OpenCode Memory Explorer"</h1>
                <ThemeToggle theme=theme/>
            </header>

            <nav class="app-nav" attr:aria-label="Primary">
                <ul>
                    {nav_entries()
                        .into_iter()
                        .map(|entry| {
                            let planned = matches!(entry.state, ScreenState::Planned { .. });
                            view! {
                                <li>
                                    <A href=entry.path>{entry.label}</A>
                                    {planned
                                        .then(|| view! { <span class="nav-badge">"planned"</span> })}
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            </nav>

            <main class="app-main">
                {move || match current_route.get() {
                    Some(id) => {
                        let label = route_spec(id).label;
                        view! {
                            <Title text=label/>
                            <ScreenBody route=id state=state_for(id)/>
                        }
                            .into_any()
                    }
                    None => {
                        view! {
                            <Title text="Unknown route"/>
                            <section class="unknown-route" data-testid="unknown-route">
                                <h2>"Unknown route"</h2>
                                <p>
                                    "This path is not a client route. Use the navigation above."
                                </p>
                            </section>
                        }
                            .into_any()
                    }
                }}
            </main>
        </div>
    }
}

/// The light/dark toggle. The preference is persisted by the `Shell` effect.
#[component]
fn ThemeToggle(theme: RwSignal<Theme>) -> impl IntoView {
    let toggle = move |_| {
        theme.update(|current| {
            *current = match *current {
                Theme::Light => Theme::Dark,
                Theme::Dark => Theme::Light,
            };
        });
    };

    view! {
        <button class="theme-toggle" type="button" on:click=toggle>
            {move || match theme.get() {
                Theme::Light => "Switch to dark",
                Theme::Dark => "Switch to light",
            }}
        </button>
    }
}
