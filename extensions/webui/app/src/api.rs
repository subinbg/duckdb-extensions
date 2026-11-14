//! The one call to the extension. Every request carries the access token.

use std::sync::OnceLock;

use gloo_net::http;
use leptos::prelude::window;
use serde::de::DeserializeOwned;
use wasm_bindgen::JsValue;
use webui_protocol::{ErrorBody, Request, TOKEN_HEADER};

const TOKEN_KEY: &str = "webui-token";

static TOKEN: OnceLock<String> = OnceLock::new();

/// Take the token from the page's address, or from this tab's storage after a reload.
///
/// The token is removed from the address bar, so that a copied or bookmarked address does not carry it.
pub fn adopt_token() -> bool {
    let storage = window().session_storage().ok().flatten();
    let search = window().location().search().unwrap_or_default();
    let in_address = search.trim_start_matches('?').split('&').find_map(|pair| pair.strip_prefix("token="));

    let token = match (in_address, &storage) {
        (Some(token), storage) => {
            let _ = storage.as_ref().map(|storage| storage.set_item(TOKEN_KEY, token));
            let _ = window().history().map(|history| history.replace_state_with_url(&JsValue::NULL, "", Some("/")));
            Some(token.to_string())
        }
        (None, Some(storage)) => storage.get_item(TOKEN_KEY).ok().flatten(),
        (None, None) => None,
    };
    token.is_some_and(|token| TOKEN.set(token).is_ok())
}

/// Send one request and read its answer as the type the protocol names beside it.
pub async fn ask<T: DeserializeOwned>(request: Request) -> Result<T, String> {
    let token = TOKEN.get().map(String::as_str).unwrap_or_default();
    let request = http::Request::post("/api").header(TOKEN_HEADER, token).json(&request);

    let unanswered = |_| "the server did not answer; is the DuckDB process still running?".to_string();
    let response = request.map_err(|error| error.to_string())?.send().await.map_err(unanswered)?;
    let text = response.text().await.map_err(|error| error.to_string())?;

    match response.ok() {
        true => serde_json::from_str(&text).map_err(|error| format!("unexpected answer from the server: {error}")),
        false => {
            Err(serde_json::from_str(&text).map_or(format!("HTTP {}", response.status()), |body: ErrorBody| body.error))
        }
    }
}

/// Cancel the running cell and whatever is computed in the background.
pub fn interrupt() {
    leptos::task::spawn_local(async { drop(ask::<()>(Request::Interrupt).await) });
}

/// Put text on the clipboard, which a browser offers only to a page from this machine or served over HTTPS.
pub fn copy(text: &str) {
    let clipboard = window().navigator().clipboard();
    if !clipboard.is_undefined() {
        let _ = clipboard.write_text(text);
    }
}
