#![cfg(target_arch = "wasm32")]
use serde_json::Value;
use std::{
    sync::{Arc, Once},
    time::Duration,
};
use tracing::info;
use wasm_bindgen_test::{Criterion, wasm_bindgen_bench};
use wasmi_plugin_hdk::{
    plugin::{Plugin, PluginId},
    server::HostServer,
};

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

const PLUGIN_WASM: &[u8] = include_bytes!("../../../target/wasm32-wasip1/release/test-plugin.wasm");

static INIT: Once = Once::new();

fn load_plugin_wasm() -> Vec<u8> {
    PLUGIN_WASM.to_vec()
}

fn get_host_server() -> HostServer<(Option<PluginId>, ())> {
    HostServer::default()
        .with_method(
            "ping",
            |_, _params: ()| async move { Ok("pong".to_string()) },
        )
        .with_method("echo", |_, params: Value| async move { Ok(params) })
}

fn setup_logs() {
    INIT.call_once(|| {
        // tracing_wasm::set_as_global_default_with_config(
        //     tracing_wasm::WASMLayerConfigBuilder::new()
        //         .set_console_config(tracing_wasm::ConsoleConfig::ReportWithoutConsoleColor)
        //         .build(),
        // );
    });
}

#[wasm_bindgen_bench]
async fn bench_ping_wasm(c: &mut Criterion) {
    setup_logs();
    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());
    let plugin = Arc::new(
        Plugin::new("test_plugin", &wasm_bytes, handler)
            .await
            .unwrap(),
    );

    c.bench_async_function("ping", |b| {
        let plugin = plugin.clone();
        Box::pin(b.iter_future(move || {
            let plugin = plugin.clone();
            async move {
                plugin.call("ping", serde_json::Value::Null).await.unwrap();
            }
        }))
    })
    .await;
}

/// Benchmark the full lifecycle of creating a plugin instance and calling a ping request.
/// Tests the overhead of plugin instantiation.
#[wasm_bindgen_bench]
async fn bench_lifecycle(c: &mut Criterion) {
    setup_logs();
    let wasm_bytes = Arc::new(load_plugin_wasm());
    let handler = Arc::new(get_host_server());

    c.bench_async_function("lifecycle", |b| {
        let wasm_bytes = wasm_bytes.clone();
        let handler = handler.clone();
        Box::pin(b.iter_future(move || {
            let wasm_bytes = wasm_bytes.clone();
            let handler = handler.clone();
            async move {
                let plugin = Plugin::new("test_plugin", &wasm_bytes, handler)
                    .await
                    .unwrap();
                plugin.call("ping", serde_json::Value::Null).await.unwrap();
            }
        }))
    })
    .await;
}

/// Benchmark the prime sieve function with a small input. Primarily tests the overhead
/// of calling into the wasm module.
#[wasm_bindgen_bench]
async fn bench_prime_sieve_small(c: &mut Criterion) {
    setup_logs();
    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());
    let plugin = Arc::new(
        Plugin::new("test_plugin", &wasm_bytes, handler)
            .await
            .unwrap(),
    );

    c.bench_async_function("prime_sieve_small", |b| {
        let plugin = plugin.clone();
        Box::pin(b.iter_future(move || {
            let plugin = plugin.clone();
            async move {
                plugin
                    .call("prime_sieve", Value::Number(1.into()))
                    .await
                    .unwrap();
            }
        }))
    })
    .await;
}

/// Benchmark the prime sieve function with a large input. Tests performance within the wasm module.
#[wasm_bindgen_bench]
async fn bench_prime_sieve_large(c: &mut Criterion) {
    setup_logs();
    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());
    let plugin = Arc::new(
        Plugin::new("test_plugin", &wasm_bytes, handler)
            .await
            .unwrap(),
    );

    c.bench_async_function("prime_sieve_large", |b| {
        let plugin = plugin.clone();
        Box::pin(b.iter_future(move || {
            let plugin = plugin.clone();
            async move {
                plugin
                    .call("prime_sieve", Value::Number(1_000_000.into()))
                    .await
                    .unwrap();
            }
        }))
    })
    .await;
}

/// Benchmark sending many echo requests to the host, and receiving responses.
#[wasm_bindgen_bench]
async fn bench_call_many(c: &mut Criterion) {
    setup_logs();

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());
    let plugin = Arc::new(
        Plugin::new("test_plugin", &wasm_bytes, handler)
            .await
            .unwrap()
            .with_timeout(Duration::from_secs(5)),
    );

    c.bench_async_function("call_many", |b| {
        let plugin = plugin.clone();
        Box::pin(b.iter_future(move || {
            let plugin = plugin.clone();
            async move {
                info!("Starting call_many benchmark iteration...");
                plugin
                    .call("call_many", Value::Number(200.into()))
                    .await
                    .unwrap();
            }
        }))
    })
    .await;
}

#[wasm_bindgen_bench]
async fn bench_call_many_async(c: &mut Criterion) {
    setup_logs();

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());
    let plugin = Arc::new(
        Plugin::new("test_plugin", &wasm_bytes, handler)
            .await
            .unwrap()
            .with_timeout(Duration::from_secs(5)),
    );

    c.bench_async_function("call_many_async", |b| {
        let plugin = plugin.clone();
        Box::pin(b.iter_future(move || {
            let plugin = plugin.clone();
            async move {
                info!("Starting call_many_async benchmark iteration...");
                plugin
                    .call("call_many_async", Value::Number(200.into()))
                    .await
                    .unwrap();
            }
        }))
    })
    .await;
}
