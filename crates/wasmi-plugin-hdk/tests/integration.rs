use serde_json::Value;
use std::sync::{Arc, Once};
use tracing::info;
use wasmi_plugin_hdk::{plugin::Plugin, server::HostServer};

#[cfg(target_family = "wasm")]
use wasm_bindgen_test::*;

#[cfg(target_family = "wasm")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

const PLUGIN_WASM: &[u8] = include_bytes!("../../../target/wasm32-wasip1/release/test-plugin.wasm");

static INIT: Once = Once::new();

fn load_plugin_wasm() -> Vec<u8> {
    PLUGIN_WASM.to_vec()
}

fn get_host_server() -> HostServer<()> {
    HostServer::default()
        .with_method("ping", |_, _params: ()| async move {
            info!("Received ping request, sending pong response");
            Ok("pong".to_string())
        })
        .with_method("echo", |_, params: Value| async move {
            info!("Received echo request, returning response");
            Ok(params)
        })
}

fn setup_logs() {
    #[cfg(not(target_family = "wasm"))]
    {
        INIT.call_once(|| {
            tracing_subscriber::fmt()
                .compact()
                .with_ansi(false)
                .with_max_level(tracing_subscriber::filter::LevelFilter::INFO)
                .with_test_writer()
                .init();
        });
    }

    #[cfg(target_family = "wasm")]
    {
        INIT.call_once(|| {
            tracing_wasm::set_as_global_default_with_config(
                tracing_wasm::WASMLayerConfigBuilder::new()
                    .set_console_config(tracing_wasm::ConsoleConfig::ReportWithoutConsoleColor)
                    .build(),
            );
        });
    }
}

#[cfg_attr(target_family = "wasm", wasm_bindgen_test)]
#[cfg_attr(not(target_family = "wasm"), tokio::test)]
async fn test_plugin() {
    setup_logs();
    info!("Starting test_plugin...");

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());

    let plugin = Plugin::new("test_plugin", wasm_bytes, handler).unwrap();
    let resp = plugin.call("ping", Value::Null).await.unwrap();
    assert_eq!(resp.result.as_str().unwrap(), "pong");
}

#[cfg_attr(target_family = "wasm", wasm_bindgen_test)]
#[cfg_attr(not(target_family = "wasm"), tokio::test)]
async fn test_get_random_number() {
    setup_logs();
    info!("Starting get_random_number test...");

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());

    let plugin = Plugin::new("test_plugin", wasm_bytes, handler).unwrap();
    let response = plugin.call("get_random_number", Value::Null).await.unwrap();

    info!("Random number response: {:?}", response);
    response.result.as_u64().unwrap();
}

#[cfg_attr(target_family = "wasm", wasm_bindgen_test)]
#[cfg_attr(not(target_family = "wasm"), tokio::test)]
async fn test_get_time() {
    setup_logs();
    info!("Starting get_time test...");

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());

    let plugin = Plugin::new("test_plugin", wasm_bytes, handler).unwrap();
    let response = plugin.call("get_time", Value::Null).await.unwrap();

    info!("Get time response: {:?}", response);

    let timestamp = response.result.as_u64().unwrap();
    let now = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let diff = timestamp.abs_diff(now);
    assert!(diff < 2, "Timestamp difference too large: {}", diff);
}

// TODO: Fix this for `wasm-pack test --headless --firefox ./crates/wasmi-plugin-hdk`
// #[cfg_attr(target_family = "wasm", wasm_bindgen_test)]
#[cfg_attr(not(target_family = "wasm"), tokio::test)]
async fn test_sleep() {
    setup_logs();
    info!("Starting sleep test...");

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());

    let plugin = Plugin::new("test_plugin", wasm_bytes, handler).unwrap();

    let sleep_duration = 1500; // milliseconds
    let start = web_time::Instant::now();
    plugin
        .call("sleep", Value::Number(sleep_duration.into()))
        .await
        .unwrap();
    let elapsed = start.elapsed().as_millis();

    assert!(
        elapsed >= sleep_duration as u128,
        "Sleep duration too short: {} ms",
        elapsed
    );
}

#[cfg_attr(target_family = "wasm", wasm_bindgen_test)]
#[cfg_attr(not(target_family = "wasm"), tokio::test)]
async fn test_call() {
    setup_logs();
    info!("Starting call test...");

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());

    let plugin = Plugin::new("test_plugin", wasm_bytes, handler).unwrap();
    let resp = plugin.call("call", Value::Null).await.unwrap();

    assert_eq!(resp.result.as_str().unwrap(), "pong");
}

#[cfg_attr(target_family = "wasm", wasm_bindgen_test)]
#[cfg_attr(not(target_family = "wasm"), tokio::test)]
async fn test_call_many() {
    setup_logs();
    info!("Starting call_many test...");

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());

    let plugin = Plugin::new("test_plugin", wasm_bytes, handler)
        .unwrap()
        .with_timeout(web_time::Duration::from_secs(2));
    plugin
        .call("call_many", Value::Number(500.into()))
        .await
        .unwrap();
}

#[cfg_attr(target_family = "wasm", wasm_bindgen_test)]
#[cfg_attr(not(target_family = "wasm"), tokio::test)]
async fn test_call_async() {
    setup_logs();
    info!("Starting call_async test...");

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());

    let plugin = Plugin::new("test_plugin", wasm_bytes, handler).unwrap();
    let resp = plugin.call("call_async", Value::Null).await.unwrap();
    assert_eq!(resp.result.as_str().unwrap(), "pong");
}

#[cfg_attr(target_family = "wasm", wasm_bindgen_test)]
#[cfg_attr(not(target_family = "wasm"), tokio::test)]
async fn test_call_many_async() {
    setup_logs();
    info!("Starting call_many_async test...");

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());

    let plugin = Plugin::new("test_plugin", wasm_bytes, handler)
        .unwrap()
        .with_timeout(web_time::Duration::from_secs(2));
    plugin
        .call("call_many_async", Value::Number(500.into()))
        .await
        .unwrap();
}

#[cfg_attr(target_family = "wasm", wasm_bindgen_test)]
#[cfg_attr(not(target_family = "wasm"), tokio::test)]
async fn test_prime_sieve() {
    setup_logs();
    info!("Starting prime sieve test...");

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());

    let plugin = Plugin::new("test_plugin", wasm_bytes, handler).unwrap();

    let response = plugin
        .call("prime_sieve", Value::Number(1000.into()))
        .await
        .unwrap();

    info!("Prime sieve response: {:?}", response);
    let count = response.result["count"].as_u64().unwrap();
    assert_eq!(count, 168);
}
