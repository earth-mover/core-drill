//! `core-drill <repo> web`: serve the metadata viewer (`web/`, embedded in
//! the binary) on localhost, with repo objects fetched through this
//! process's storage connection.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use color_eyre::eyre::Result;
use icechunk::Repository;

const JS: &str = "text/javascript; charset=utf-8";

const ASSETS: &[(&str, &str, &[u8])] = &[
    (
        "/",
        "text/html; charset=utf-8",
        include_bytes!("../web/index.html"),
    ),
    ("/decoder.js", JS, include_bytes!("../web/decoder.js")),
    ("/viewer.js", JS, include_bytes!("../web/viewer.js")),
    ("/schema.js", JS, include_bytes!("../web/schema.js")),
    (
        "/vendor/zstd.js",
        JS,
        include_bytes!("../web/vendor/zstd.js"),
    ),
];

struct AppState {
    repo: Repository,
    /// Accepted `Host` header values.
    hosts: Vec<String>,
}

pub async fn serve(repo: Repository, label: String, port: u16, open_browser: bool) -> Result<()> {
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await?;
    let port = listener.local_addr()?.port();
    let state = Arc::new(AppState {
        repo,
        hosts: vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")],
    });

    let config = format!(
        "window.CORE_DRILL = {{ source: \"repo/\", name: {} }};\n",
        serde_json::to_string(&label)?
    );
    let mut app = Router::new()
        .route(
            "/core-drill.js",
            get(move || async move { asset(JS, config.into_bytes()) }),
        )
        .route("/repo/{*path}", get(repo_object));
    for (route, content_type, bytes) in ASSETS {
        app = app.route(
            route,
            get(move || async move { asset(content_type, bytes.to_vec()) }),
        );
    }
    let app = app
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            check_host,
        ))
        .with_state(state);

    let url = format!("http://127.0.0.1:{port}/");
    eprintln!("Serving the viewer for {label} at {url} (Ctrl+C to stop)");
    if open_browser && let Err(e) = open::that(&url) {
        eprintln!("Could not open a browser ({e}); visit {url}");
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

fn asset(content_type: &'static str, body: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

async fn repo_object(State(state): State<Arc<AppState>>, Path(path): Path<String>) -> Response {
    match crate::fetch::raw::fetch_bytes(&state.repo, &path).await {
        Ok(bytes) => asset("application/octet-stream", bytes),
        Err(e) => {
            let message = e.to_string();
            let first_line = message.lines().next().unwrap_or_default();
            (StatusCode::NOT_FOUND, crate::sanitize::sanitize(first_line)).into_response()
        }
    }
}

/// Reject requests whose `Host` isn't this server, so a page on another site
/// can't read repo data through DNS rebinding.
async fn check_host(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok());
    if host.is_some_and(|h| state.hosts.iter().any(|ok| ok == h)) {
        return next.run(request).await;
    }
    let mut response = (StatusCode::FORBIDDEN, "unexpected Host header").into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
