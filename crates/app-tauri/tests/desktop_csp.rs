//! Guard rails around the desktop webview content-security policy (ADR-012).
//!
//! CesiumJS needs `'wasm-unsafe-eval'` (it instantiates WebAssembly while the
//! module loads) and `'unsafe-eval'` (knockout, used by `Cesium.Viewer`'s DOM,
//! compiles `data-bind` expressions with `new Function`). Without them the
//! bundle throws during evaluation, React never mounts, and the window shows
//! only the flat background colour — the "black window" symptom documented in
//! ADR-012. These tests fail loudly if the policy is tightened again.

use serde_json::Value;
use std::fs;

fn config() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json");
    let raw = fs::read_to_string(path).expect("tauri.conf.json is readable");
    serde_json::from_str(&raw).expect("tauri.conf.json is valid JSON")
}

fn csp() -> String {
    config()["app"]["security"]["csp"]
        .as_str()
        .expect("app.security.csp is configured as a string")
        .to_string()
}

fn directive(csp: &str, name: &str) -> String {
    let prefix = format!("{name} ");
    csp.split(';')
        .map(str::trim)
        .find(|item| *item == name || item.starts_with(&prefix))
        .unwrap_or_default()
        .to_string()
}

#[test]
fn script_src_allows_the_cesiumjs_runtime() {
    let script_src = directive(&csp(), "script-src");
    for required in ["'self'", "'wasm-unsafe-eval'", "'unsafe-eval'"] {
        assert!(
            script_src.contains(required),
            "script-src must keep {required} for CesiumJS (ADR-012); got: {script_src}"
        );
    }
}

#[test]
fn script_src_forbids_inline_script() {
    let script_src = directive(&csp(), "script-src");
    assert!(
        !script_src.contains("'unsafe-inline'"),
        "inline script must stay forbidden; got: {script_src}"
    );
}

#[test]
fn worker_src_allows_blob_workers() {
    let worker_src = directive(&csp(), "worker-src");
    for required in ["'self'", "blob:"] {
        assert!(
            worker_src.contains(required),
            "worker-src must keep {required} for Cesium workers; got: {worker_src}"
        );
    }
}

#[test]
fn script_src_has_no_remote_origins() {
    let script_src = directive(&csp(), "script-src");
    let origins: Vec<&str> = script_src
        .split_whitespace()
        .filter(|token| !token.starts_with('\'') && *token != "script-src")
        .collect();
    assert!(
        origins.is_empty(),
        "script-src must not allow remote origins; got: {origins:?}"
    );
}
