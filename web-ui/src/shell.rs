//! The shell view: `ConfigProvider` + `Router` + the layout.
//!
//! `use_location()` is called by `ShellView`, which is rendered *inside* the
//! `<Router>` subtree. Calling it in a parent of the router panics at mount and
//! leaves an empty `<body>`.

use leptos::prelude::*;
use leptos_meta::{Title, provide_meta_context};
use leptos_router::NavigateOptions;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::hooks::use_navigate;
use leptos_router::path;
use thaw::{
    Button, ButtonAppearance, ConfigProvider, Layout, LayoutHeader, LayoutSider, NavDrawer,
    NavDrawerHeader, NavItem,
};

use crate::nav::{ScreenState, is_plain_left_click, nav_entries, selected_path, state_for};
use crate::routes::{resolve, route as route_spec};
use crate::screens::ScreenBody;
use crate::theme::{
    LocalStorageThemeStore, Scale, Theme, apply_canvas, install_tokens, palette, read_preference,
    store_preference, thaw_theme,
};

/// The application shell.
#[component]
pub fn Shell() -> impl IntoView {
    provide_meta_context();

    let theme = RwSignal::new(read_preference(&LocalStorageThemeStore));
    provide_context(theme);
    install_tokens(&Scale::default());

    // thaw's theme is a separate signal, kept in sync with the preference (D6).
    let thaw_signal: RwSignal<thaw::Theme> = RwSignal::new(thaw_theme(theme.get_untracked()));
    Effect::new(move |_| {
        let current = theme.get();
        thaw_signal.set(thaw_theme(current));
        apply_canvas(&palette(current));
        store_preference(&LocalStorageThemeStore, current);
    });

    view! {
        <ConfigProvider theme=thaw_signal>
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
    let navigate = use_navigate();
    let pathname = location.pathname;

    // The route is the truth: NavItem sets this on click, and this effect
    // re-asserts it from the pathname (and restores `aria-current`, which
    // Thaw's NavItem does not forward).
    let selected: RwSignal<Option<String>> =
        RwSignal::new(selected_path(&pathname.get_untracked()).map(str::to_string));
    Effect::new(move |_| {
        let path = pathname.get();
        let route_path = selected_path(&path);
        selected.set(route_path.map(str::to_string));
        let doc = document();
        if let Ok(Some(stale)) = doc.query_selector("a[aria-current]") {
            let _ = stale.remove_attribute("aria-current");
        }
        let query = format!(
            "a.thaw-nav-item[href=\"{}\"]",
            route_path.unwrap_or_default()
        );
        if let Ok(Some(link)) = doc.query_selector(&query) {
            let _ = link.set_attribute("aria-current", "page");
        }
    });

    let toggle = move |_| {
        theme.update(|current| {
            *current = match *current {
                Theme::Light => Theme::Dark,
                Theme::Dark => Theme::Light,
            };
        });
    };

    view! {
        <Layout class="app-shell" content_class="app-layout-row">
            <LayoutSider class="app-sider">
                <nav aria-label="Primary">
                    <NavDrawer class="app-nav-drawer" selected_value=selected>
                        <NavDrawerHeader slot>
                            <span class="app-brand">"OpenCode Memory Explorer"</span>
                        </NavDrawerHeader>
                        {nav_entries()
                            .into_iter()
                            .map(|entry| {
                                let navigate = navigate.clone();
                                let planned = matches!(entry.state, ScreenState::Planned { .. });
                                view! {
                                    <div on:click=move |ev: leptos::ev::MouseEvent| {
                                        if is_plain_left_click(
                                            ev.button(),
                                            ev.ctrl_key(),
                                            ev.meta_key(),
                                            ev.shift_key(),
                                            ev.alt_key(),
                                        ) {
                                            ev.prevent_default();
                                            navigate(entry.path, NavigateOptions::default());
                                        } else {
                                            // A modified click does not change the route, so
                                            // the pathname effect would never undo Thaw's own
                                            // selection. Re-assert it here (this handler is on
                                            // the parent, so it runs second).
                                            selected
                                                .set(
                                                    selected_path(&pathname.get_untracked())
                                                        .map(str::to_string),
                                                );
                                        }
                                    }>
                                        <NavItem
                                            value=Signal::derive(move || entry.path.to_string())
                                            href=Signal::derive(move || entry.path.to_string())
                                        >
                                            {entry.label}
                                            {planned
                                                .then(|| view! { <span class="nav-badge">"planned"</span> })}
                                        </NavItem>
                                    </div>
                                }
                            })
                            .collect_view()}
                    </NavDrawer>
                </nav>
            </LayoutSider>
            <div class="app-body">
                <LayoutHeader class="app-header">
                    <h1 class="app-title">
                        {move || current_route.get().map(|id| route_spec(id).label)}
                    </h1>
                    <Button appearance=ButtonAppearance::Subtle on_click=toggle>
                        {move || match theme.get() {
                            Theme::Light => "Switch to dark",
                            Theme::Dark => "Switch to light",
                        }}
                    </Button>
                </LayoutHeader>
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
        </Layout>
    }
}
