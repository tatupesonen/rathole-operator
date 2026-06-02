//! rathole-config-receiver — runs on the VPS next to the rathole server.
//!
//! Accepts an authenticated `POST /config` with the rendered `server.toml` and
//! writes it **in place** (no rename). rathole's hot-reload watcher only honors
//! `Modify` (write) events on the config filename and ignores rename/move, so an
//! in-place overwrite is required to trigger a reload.
//!
//! Env:
//!   RECEIVER_TOKEN   (required)  bearer token clients must present
//!   RECEIVER_LISTEN  default 0.0.0.0:2334
//!   RECEIVER_OUTPUT  default /config/server.toml

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use tracing::{error, info};

struct Cfg {
    token: String,
    output: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let token = std::env::var("RECEIVER_TOKEN").unwrap_or_else(|_| {
        error!("RECEIVER_TOKEN is required");
        std::process::exit(1);
    });
    let listen = std::env::var("RECEIVER_LISTEN").unwrap_or_else(|_| "0.0.0.0:2334".into());
    let output = std::env::var("RECEIVER_OUTPUT").unwrap_or_else(|_| "/config/server.toml".into());

    let cfg = Arc::new(Cfg { token, output });
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/config", post(put_config))
        .with_state(cfg);

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .unwrap_or_else(|e| {
            error!("bind {listen} failed: {e}");
            std::process::exit(1);
        });
    info!("rathole-config-receiver listening on {listen}");
    axum::serve(listener, app).await.unwrap();
}

async fn put_config(State(cfg): State<Arc<Cfg>>, headers: HeaderMap, body: Bytes) -> StatusCode {
    if !authorized(&headers, &cfg.token) {
        return StatusCode::UNAUTHORIZED;
    }
    match write_in_place(&cfg.output, &body) {
        Ok(()) => {
            info!("wrote {} ({} bytes)", cfg.output, body.len());
            StatusCode::NO_CONTENT
        }
        Err(e) => {
            error!("write {} failed: {e}", cfg.output);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

fn authorized(headers: &HeaderMap, expected: &str) -> bool {
    let Some(value) = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(presented) = value.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(presented.as_bytes(), expected.as_bytes())
}

/// Overwrite the file from offset 0 then trim to the new length. Never empties
/// the file (unlike truncate-then-write), and avoids rename (which rathole's
/// watcher would ignore).
fn write_in_place(path: &str, body: &[u8]) -> std::io::Result<()> {
    // truncate(false) is deliberate: overwrite from offset 0, then set_len to
    // trim — so the file is never momentarily empty (a truncate-on-open would
    // race rathole's watcher into reading an empty config).
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    f.write_all(body)?;
    f.set_len(body.len() as u64)?;
    f.flush()?;
    f.sync_all()?;
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}
