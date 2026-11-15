//! CMD-THUMB: the `sheaf-thumb://` scheme, page pictures as bytes with no
//! base64 and no port (PERF-IO, ADR-NOHTTP).
//!
//! The URL path is `/<pageId>/<ver>/<size>`: `ver` fingerprints the page's
//! edits, so an edit changes the URL and only that page's picture reloads;
//! `size` is the longest side in pixels (small is a thumbnail, large a preview).

use crate::shell::Shell;
use std::borrow::Cow;
use tauri::http::{Response, StatusCode};
use tauri::{Manager, UriSchemeContext, UriSchemeResponder, Wry};

type Reply = Response<Cow<'static, [u8]>>;

fn reply(status: StatusCode, content_type: &str, body: Vec<u8>) -> Reply {
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        // The version in the URL changes when the picture would.
        .header("Cache-Control", "private, max-age=86400, immutable")
        .body(Cow::Owned(body))
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
}

fn status_for(code: sheaf_core::ErrorCode) -> StatusCode {
    use sheaf_core::ErrorCode::*;
    match code {
        NotFound => StatusCode::NOT_FOUND,
        InvalidArg | LimitExceeded => StatusCode::BAD_REQUEST,
        Cancelled => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub fn handle(ctx: UriSchemeContext<'_, Wry>, request: tauri::http::Request<Vec<u8>>, responder: UriSchemeResponder) {
    let app = ctx.app_handle().clone();
    let path = request.uri().path().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let response = match (
            sheaf_app::api::parse_thumb_path(&path),
            app.try_state::<Shell>().map(|s| s.backend()),
        ) {
            (Some((page, size)), Some(Ok(backend))) => match backend.thumb(page, size) {
                Ok(png) => reply(StatusCode::OK, "image/png", png),
                Err(e) => reply(status_for(e.code), "text/plain", Vec::new()),
            },
            (None, _) => reply(StatusCode::BAD_REQUEST, "text/plain", Vec::new()),
            _ => reply(StatusCode::SERVICE_UNAVAILABLE, "text/plain", Vec::new()),
        };
        responder.respond(response);
    });
}
