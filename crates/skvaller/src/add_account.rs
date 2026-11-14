//! Adding an account: the services to add one for, then the chosen one's sign-in form.
//!
//! In the Settings window, on the main window while there is no account, and filled in
//! to sign in again once the server rejected an account's token.

use std::sync::Arc;

use dioxus_native::prelude::*;
use skvaller_app::{Account, AccountKind, App};

use crate::chrome::{DragArea, MAC};
use crate::i18n::{sentence, t, t_with};
use crate::icons::{CHEVRON_LEFT, CHEVRON_RIGHT, Icon, LAPTOP, RSS};

const ADD_ACCOUNT_CSS: &str = include_str!("add_account.css");

/// A sync service the list offers. All speak the Google Reader API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    TheOldReader,
    FreshRss,
    Miniflux,
    /// Any other server with the Google Reader API.
    Other,
}

const HOSTED: [Service; 1] = [Service::TheOldReader];
const SELF_HOSTED: [Service; 3] = [Service::FreshRss, Service::Miniflux, Service::Other];

impl Service {
    fn name(self) -> String {
        match self {
            Service::TheOldReader => "The Old Reader".into(),
            Service::FreshRss => "FreshRSS".into(),
            Service::Miniflux => "Miniflux".into(),
            Service::Other => t("services.otherName"),
        }
    }

    /// Under the name in the list.
    fn detail(self) -> &'static str {
        match self {
            Service::TheOldReader => "theoldreader.com",
            Service::FreshRss => "freshrss.org",
            Service::Miniflux => "miniflux.app",
            Service::Other => "Google Reader API",
        }
    }

    /// The help on how to get a password the API takes.
    fn help(self) -> String {
        let id = match self {
            Service::TheOldReader => "theoldreader",
            Service::FreshRss => "freshrss",
            Service::Miniflux => "miniflux",
            Service::Other => "other",
        };
        t(&format!("services.{id}.help"))
    }

    /// The API address of a hosted service: its form doesn't ask for one.
    fn server(self) -> Option<&'static str> {
        (self == Service::TheOldReader).then_some("https://theoldreader.com")
    }

    /// What the service calls the user name, as in the `signIn.*` messages.
    fn user_field(self) -> &'static str {
        match self {
            Service::TheOldReader => "email",
            _ => "username",
        }
    }

    /// What the service calls the password, as in the `signIn.*` messages.
    fn password_field(self) -> &'static str {
        match self {
            Service::TheOldReader | Service::Miniflux => "password",
            Service::FreshRss | Service::Other => "apiPassword",
        }
    }

    /// The service an account's saved API address belongs to, to sign in to it again.
    fn for_server(server: &str) -> Self {
        let rest = server.split_once("://").map_or(server, |(_, rest)| rest);
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        if host == "theoldreader.com" {
            Service::TheOldReader
        } else if path.trim_end_matches('/').ends_with(&FRESHRSS_API[1..]) {
            Service::FreshRss
        } else {
            Service::Other
        }
    }
}

/// Where `FreshRSS` keeps its Google Reader API, under the server's address.
const FRESHRSS_API: &str = "/api/greader.php";

/// The address as typed, with `https://` when it has no scheme and no `/` at the end.
/// For `FreshRSS`, its API's: a server may take long to answer a sign-in anywhere else.
fn server_url(service: Service, typed: &str) -> String {
    let typed = typed.trim();
    let url = if typed.contains("://") {
        typed.to_owned()
    } else {
        format!("https://{typed}")
    };
    let url = url.trim_end_matches('/');
    if service == Service::FreshRss && !url.ends_with(FRESHRSS_API) {
        format!("{url}{FRESHRSS_API}")
    } else {
        url.to_owned()
    }
}

/// Why signing in failed, in the service's own words for its user name and password.
fn sign_in_error(service: Service, e: &skvaller_app::Error) -> String {
    use skvaller_core::Error as Core;
    match e {
        skvaller_app::Error::Core(Core::Unauthorized) => t(&format!(
            "signIn.wrong.{}.{}",
            service.user_field(),
            service.password_field()
        )),
        skvaller_app::Error::Core(Core::BadUrl) => t("signIn.badUrl"),
        skvaller_app::Error::Core(Core::Http(_)) if service.server().is_some() => {
            t_with("signIn.networkService", &[("service", &service.name())])
        }
        skvaller_app::Error::Core(Core::Http(_)) => t("signIn.networkServer"),
        e => sentence(&e.to_string()),
    }
}

/// The list of services, then the chosen one's sign-in form. `again` is an account to
/// sign in to again: then only its form shows. `on_added` gets the account's ID.
#[component]
pub fn AddAccount(
    /// In the Settings window, rather than the whole main window.
    embedded: bool,
    again: Option<Account>,
    /// Over the list, or in the form of the account to sign in to again.
    initial_error: Option<String>,
    on_added: EventHandler<String>,
) -> Element {
    let app = use_context::<Arc<App>>();
    let mut chosen = use_signal(|| None::<Service>);
    let mut local_error = use_signal(|| None::<String>);
    let has_local = app.accounts().iter().any(|a| a.kind == AccountKind::Local);
    let again_service = again.as_ref().and_then(|a| match &a.kind {
        AccountKind::Greader { server, .. } => Some(Service::for_server(server)),
        AccountKind::Local => None,
    });

    rsx! {
        style { {ADD_ACCOUNT_CSS} }
        if !embedded && MAC {
            // The title bar is hidden: an empty strip to drag the window by, over
            // the top of the screen only, so the rest stays selectable and scrollable.
            div { class: "drag-strip", DragArea {} }
        }
        div { class: if embedded { "add-account embedded" } else { "add-account" },
            div { class: "column",
                if let Some(service) = again_service {
                    SignIn { service, again, initial_error, embedded, on_added }
                } else if let Some(service) = chosen() {
                    SignIn {
                        service,
                        embedded,
                        back: move |()| chosen.set(None),
                        on_added,
                    }
                } else {
                    if !embedded {
                        h1 { {t("addAccount.title")} }
                        p { class: "intro", {t("addAccount.intro")} }
                    }
                    if let Some(e) = initial_error {
                        p { class: "error", role: "alert", "{e}" }
                    }
                    div { class: "groups",
                        section {
                            h2 { class: "heading", {t("addAccount.services")} }
                            for service in HOSTED {
                                ServiceRow { key: "{service:?}", service, onclick: move |()| chosen.set(Some(service)) }
                            }
                        }
                        section {
                            h2 { class: "heading", {t("addAccount.selfHosted")} }
                            for service in SELF_HOSTED {
                                ServiceRow { key: "{service:?}", service, onclick: move |()| chosen.set(Some(service)) }
                            }
                        }
                        if !has_local {
                            section {
                                h2 { class: "heading", {t("addAccount.withoutAccount")} }
                                button {
                                    class: "service",
                                    onclick: move |_| match app.add_local_account() {
                                        Ok(id) => on_added.call(id),
                                        Err(e) => local_error.set(Some(sentence(&e.to_string()))),
                                    },
                                    Icon { size: 27, stroke: "1.5", paths: LAPTOP }
                                    span { class: "text",
                                        strong { {t("addAccount.local")} }
                                        small { {t("addAccount.localDetail")} }
                                    }
                                    span { class: "chevron", Icon { size: 16, stroke: "2", paths: CHEVRON_RIGHT } }
                                }
                                if let Some(e) = local_error() {
                                    p { class: "error", role: "alert", "{e}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ServiceRow(service: Service, onclick: EventHandler<()>) -> Element {
    rsx! {
        button { class: "service", onclick: move |_| onclick.call(()),
            Logo { service, size: 32 }
            span { class: "text",
                strong { {service.name()} }
                small { {service.detail()} }
            }
            span { class: "chevron", Icon { size: 16, stroke: "2", paths: CHEVRON_RIGHT } }
        }
    }
}

/// The sign-in form of `service`, filled in from `again` to sign in to it again.
#[component]
fn SignIn(
    service: Service,
    again: Option<Account>,
    initial_error: Option<String>,
    embedded: bool,
    /// Back to the list; none when signing in again.
    back: Option<EventHandler<()>>,
    on_added: EventHandler<String>,
) -> Element {
    let app = use_context::<Arc<App>>();
    let (typed_server, typed_username) = match again.as_ref().map(|a| &a.kind) {
        Some(AccountKind::Greader { server, username }) => (server.clone(), username.clone()),
        _ => (String::new(), String::new()),
    };
    let mut server = use_signal(|| typed_server);
    let mut username = use_signal(|| typed_username);
    let mut password = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| initial_error.clone());
    let submit = use_callback(move |()| {
        if *busy.peek() {
            return;
        }
        let address = if let Some(address) = service.server() {
            address.to_owned()
        } else {
            let address = server_url(service, &server.peek());
            server.set(address.clone());
            address
        };
        let (user, secret) = (username.peek().clone(), password.peek().clone());
        busy.set(true);
        error.set(None);
        let app = Arc::clone(&app);
        spawn(async move {
            let signed_in = app.sign_in(&address, &user, &secret).await;
            busy.set(false);
            match signed_in {
                Ok(id) => {
                    password.set(String::new());
                    on_added.call(id);
                }
                Err(e) => error.set(Some(sign_in_error(service, &e))),
            }
        });
    });
    // Keys typed in a field are the form's: Enter signs in, Escape goes back. With a
    // modifier, as the app's own shortcuts, they are the window's too.
    let on_key = move |e: Event<KeyboardData>| {
        let key = e.key();
        let shortcut = e
            .modifiers()
            .intersects(Modifiers::CONTROL | Modifiers::META | Modifiers::ALT);
        if shortcut || (key == Key::Named(NamedKey::Escape) && back.is_none()) {
            return;
        }
        e.stop_propagation();
        match key {
            Key::Named(NamedKey::Enter) => submit.call(()),
            Key::Named(NamedKey::Escape) if !*busy.peek() => {
                if let Some(back) = back {
                    back.call(());
                }
            }
            _ => {}
        }
    };
    let title = again
        .as_ref()
        .map_or_else(|| service.name(), |a| a.description.clone());
    // The first field to fill in has the focus.
    let first = if again.is_some() {
        "password"
    } else if service.server().is_none() {
        "server"
    } else {
        "username"
    };
    let focus = move |field: &'static str| {
        move |e: MountedEvent| async move {
            if field == first
                && let Err(err) = e.data().set_focus(true).await
            {
                tracing::warn!("focusing the {field} field failed: {err}");
            }
        }
    };
    let hint = t_with(
        "common.twoSentences",
        &[
            ("first", &service.help()),
            ("second", &t("signIn.tokenOnly")),
        ],
    );
    let is_busy = busy();

    rsx! {
        div { class: "sign-in",
            header {
                if let Some(back) = back {
                    button {
                        class: "back",
                        aria_label: t("signIn.allServices"),
                        disabled: is_busy,
                        onclick: move |_| back.call(()),
                        Icon { size: 18, stroke: "2", paths: CHEVRON_LEFT }
                    }
                }
                Logo { service, size: if embedded { 24 } else { 28 } }
                h1 { class: if embedded { "title small" } else { "title" }, "{title}" }
            }
            if service.server().is_none() {
                div { class: "field",
                    span { {t("signIn.server")} }
                    input {
                        value: "{server}",
                        placeholder: "https://rss.example.com",
                        spellcheck: "false",
                        aria_label: t("signIn.server"),
                        oninput: move |e| server.set(e.value()),
                        onkeydown: on_key,
                        onmounted: focus("server"),
                    }
                }
            }
            div { class: "field",
                span { {t(&format!("signIn.{}", service.user_field()))} }
                input {
                    value: "{username}",
                    spellcheck: "false",
                    aria_label: t(&format!("signIn.{}", service.user_field())),
                    oninput: move |e| username.set(e.value()),
                    onkeydown: on_key,
                    onmounted: focus("username"),
                }
            }
            div { class: "field",
                span { {t(&format!("signIn.{}", service.password_field()))} }
                input {
                    r#type: "password",
                    value: "{password}",
                    aria_label: t(&format!("signIn.{}", service.password_field())),
                    oninput: move |e| password.set(e.value()),
                    onkeydown: on_key,
                    onmounted: focus("password"),
                }
            }
            p { class: "hint", "{hint}" }
            if let Some(e) = error() {
                p { class: "error", role: "alert", "{e}" }
            }
            button {
                class: "primary",
                disabled: is_busy,
                onclick: move |_| submit.call(()),
                if is_busy { {t("signIn.signingIn")} } else { {t("signIn.submit")} }
            }
        }
    }
}

/// The service's mark, in the text color; a feed icon for any other server. The marks
/// are their owners', used to name the service.
#[component]
fn Logo(service: Service, size: u32) -> Element {
    match service {
        // freshrss.org, AGPL-3.0, without its white disc.
        Service::FreshRss => rsx! {
            svg { width: "{size}", height: "{size}", view_box: "0 0 256 256", "aria-hidden": "true",
                circle { cx: "128", cy: "128", r: "33", fill: "currentColor" }
                g { fill: "none", stroke: "currentColor", stroke_width: "24",
                    g { stroke_opacity: "0.3",
                        path { d: "M12 128a116 116 0 1 1 116 116" }
                        path { d: "M54 128a74 74 0 1 1 74 74" }
                    }
                    path { d: "M128 12a116 116 0 0 1 116 116M128 54a74 74 0 0 1 74 74" }
                }
            }
        },
        // miniflux.app, Apache-2.0, without its background.
        Service::Miniflux => rsx! {
            svg { width: "{size}", height: "{size}", view_box: "37 102 432 300", "aria-hidden": "true",
                path { fill: "currentColor", d: MINIFLUX }
            }
        },
        // From github.com/theoldreader/api, media/oldreader-icon.svg.
        Service::TheOldReader => rsx! {
            svg { width: "{size}", height: "{size}", view_box: "0 0 612 612", "aria-hidden": "true",
                path { fill: "currentColor", d: THE_OLD_READER }
            }
        },
        Service::Other => rsx! {
            Icon { size: size * 85 / 100, stroke: "2", paths: RSS }
        },
    }
}

const MINIFLUX: &str = "M178.24,117a95.75,95.75,0,0,1,45.48-11q50.36,0,64.16,43.8a139.1,139.1,0,0,1,37.57-31.07q22.13-12.73,49.34-12.73,37.77,0,54,23.08T445,198.38v174c0,4.35.61,7.3,1.83,8.88s3.86,3,7.92,4.14L469,390.14v10.65H384.53q-11,0-15.83-8.29t-4.88-24.86V187.73q0-26.64-5.89-37.88T338.24,138.6q-21.93,0-46.7,26A210,210,0,0,1,294,198.38v174c0,4.35.61,7.3,1.83,8.88s3.86,3,7.92,4.14l14.21,4.74v10.65H233.47q-11,0-15.84-8.29t-4.88-24.86V187.73q0-26.64-5.88-37.88t-19.7-11.25q-21.53,0-44.26,23.68v210.1c0,4.35.6,7.4,1.82,9.17s3.72,3.26,7.52,4.44l13.8,4.15v10.65H37.73V390.14l14.21-4.74q6.09-1.77,7.92-4.14c1.22-1.58,1.83-4.53,1.83-8.88V148.67q0-6.51-1.83-8.88t-7.92-4.15l-14.21-4.73V120.26l97.46-17.76h6.9v41.43A142.53,142.53,0,0,1,178.24,117Z";

const THE_OLD_READER: &str = "M566.3,421.7l-51.1-51.1c32.7-20,54.7-56,54.7-97.1c0-62.8-51.1-113.9-113.9-113.9H350.1c-0.1,0-0.2,0-0.3,0 c-0.7,0-1.5,0.1-2.2,0.2c-0.3,0.1-0.7,0.1-1,0.2c-0.7,0.1-1.3,0.4-2,0.6c-0.4,0.1-0.7,0.2-1.1,0.4c-0.6,0.2-1.1,0.6-1.6,0.9 c-0.4,0.2-0.8,0.4-1.1,0.7c-0.1,0.1-0.3,0.2-0.4,0.3c-0.4,0.3-0.6,0.6-1,0.9c-0.3,0.3-0.7,0.6-1,0.9c-0.4,0.5-0.8,0.9-1.2,1.4 c-0.3,0.4-0.5,0.7-0.8,1.1c-0.3,0.5-0.6,1-0.9,1.6c-0.2,0.4-0.4,0.8-0.6,1.3c-0.2,0.5-0.4,1.1-0.5,1.6c-0.1,0.5-0.3,0.9-0.3,1.4 c-0.1,0.6-0.1,1.2-0.2,1.8c0,0.3-0.1,0.7-0.1,1c0,0.1,0,0.3,0,0.4c0,0.6,0.1,1.2,0.2,1.8c0.1,0.5,0.1,1,0.2,1.5 c0.1,0.5,0.3,0.9,0.4,1.4c0.2,0.6,0.4,1.1,0.6,1.7c0.2,0.4,0.4,0.7,0.6,1.1c0.3,0.6,0.7,1.2,1.1,1.7c0.1,0.1,0.1,0.2,0.2,0.3 c19.2,25.5,29.3,55.9,29.3,87.9c0,80.7-65.7,146.4-146.4,146.4S73.5,354.2,73.5,273.5c0-5.5,0.4-10.9,1-16.3h32.8 c-0.8,5.4-1.2,10.8-1.2,16.3c0,62.8,51.1,113.9,113.9,113.9s113.9-51.1,113.9-113.9s-51.1-113.9-113.9-113.9H93 c-9,0-16.3,7.3-16.3,16.3S84,192.1,93,192.1h126.9c44.9,0,81.3,36.5,81.3,81.3s-36.5,81.3-81.3,81.3s-81.3-36.5-81.3-81.3 c0-5.5,0.6-10.9,1.7-16.3H174c-1.8,5.1-2.8,10.6-2.8,16.3c0,26.9,21.9,48.8,48.8,48.8s48.8-21.9,48.8-48.8 c0-26.9-21.9-48.8-48.8-48.8H60.4c-8.1,0-14.8,6-16,13.8c-2.3,11.4-3.4,23.2-3.4,35c0,98.7,80.3,179,179,179 c81.8,0,150.8-55.1,172.1-130.2H456c26.9,0,48.8-21.9,48.8-48.8c0-26.9-21.9-48.8-48.8-48.8h-64c-3.2-11.2-7.4-22.1-12.7-32.5H456 c44.9,0,81.3,36.5,81.3,81.3s-36.5,81.3-81.3,81.3l-32.5-0.1c-9,0-16.3,7.3-16.3,16.3c0,0.1,0,0.3,0,0.4c0,0.2-0.1,0.5-0.1,0.7v64 c0,9,7.3,16.3,16.3,16.3s16.3-7.3,16.3-16.3v-48.8l16.3,0c9.2,0,18.1-1.2,26.6-3.3l60.6,60.6c3.2,3.2,7.3,4.8,11.5,4.8 s8.3-1.6,11.5-4.8C572.6,438.4,572.6,428.1,566.3,421.7z M236.2,273.5c0,9-7.3,16.3-16.3,16.3c-9,0-16.3-7.3-16.3-16.3 c0-9,7.3-16.3,16.3-16.3C228.9,257.2,236.2,264.5,236.2,273.5z M456,257.2c9,0,16.3,7.3,16.3,16.3c0,9-7.3,16.3-16.3,16.3h-57.9 c0.5-5.4,0.8-10.8,0.8-16.3c0-5.5-0.3-10.9-0.8-16.3H456z";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_address_names_its_service() {
        assert_eq!(
            Service::for_server("https://theoldreader.com"),
            Service::TheOldReader
        );
        assert_eq!(
            Service::for_server("http://192.168.1.138:8080/api/greader.php"),
            Service::FreshRss
        );
        assert_eq!(
            Service::for_server("https://rss.example.com"),
            Service::Other
        );
    }

    #[test]
    fn a_typed_address_gets_a_scheme_and_loses_its_slash() {
        assert_eq!(
            server_url(Service::Other, " rss.example.com/ "),
            "https://rss.example.com"
        );
        assert_eq!(
            server_url(Service::Miniflux, "http://10.0.0.2:8080"),
            "http://10.0.0.2:8080"
        );
        assert_eq!(
            server_url(Service::FreshRss, "http://10.0.0.2:8080/"),
            "http://10.0.0.2:8080/api/greader.php"
        );
        assert_eq!(
            server_url(Service::FreshRss, "rss.example.com/api/greader.php"),
            "https://rss.example.com/api/greader.php"
        );
    }
}
