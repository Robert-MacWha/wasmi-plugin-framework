use criterion::{Criterion, criterion_group, criterion_main};
use serde_json::Value;
use std::sync::Arc;
use tokio::runtime::Builder;
use wasmi_plugin_hdk::{
    plugin::{Plugin, PluginId},
    server::Server,
};

const PLUGIN_WASM: &[u8] = include_bytes!("../../../target/wasm32-wasip1/release/test-plugin.wasm");

fn load_plugin_wasm() -> Vec<u8> {
    PLUGIN_WASM.to_vec()
}

fn get_host_server() -> Server<(Option<PluginId>, ())> {
    Server::default().with_method("echo", |_, params: Value| async move { Ok(params) })
}

/// Benchmark a single ping request to the wasm module.
pub fn bench_ping(c: &mut Criterion) {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());
    let plugin = Plugin::new("test_plugin", wasm_bytes.clone(), handler.clone()).unwrap();

    c.bench_function("ping", |b| {
        b.iter(|| {
            let fut = async {
                plugin.call("ping", Value::Null).await.unwrap();
            };

            rt.block_on(fut);
        })
    });
}

/// Benchmark the prime sieve function with a small input. Primarily tests the overhead
/// of calling into the wasm module.
pub fn bench_prime_sieve_small(c: &mut Criterion) {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());
    let plugin = Plugin::new("test_plugin", wasm_bytes.clone(), handler).unwrap();

    c.bench_function("prime_sieve_small", |b| {
        b.iter(|| {
            let fut = async {
                plugin
                    .call("prime_sieve", Value::Number(1.into()))
                    .await
                    .unwrap();
            };

            rt.block_on(fut);
        })
    });
}

/// Benchmark the prime sieve function with a large input. Tests both the overhead
/// of calling into the wasm module and performance within the wasm module.
pub fn bench_prime_sieve_large(c: &mut Criterion) {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());
    let plugin = Plugin::new("test_plugin", wasm_bytes.clone(), handler).unwrap();

    c.bench_function("prime_sieve_large", |b| {
        b.iter(|| {
            let fut = async {
                plugin
                    .call("prime_sieve", Value::Number(100_000.into()))
                    .await
                    .unwrap();
            };

            rt.block_on(fut);
        })
    });
}

/// Benchmark sending many echo requests to the host, and receiving responses.
pub fn bench_echo_many(c: &mut Criterion) {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();

    let wasm_bytes = load_plugin_wasm();
    let handler = Arc::new(get_host_server());
    let plugin = Plugin::new("test_plugin", wasm_bytes.clone(), handler).unwrap();

    c.bench_function("many_echo", |b| {
        b.iter(|| {
            let fut = async {
                plugin
                    .call("many_echo", Value::Number(200.into()))
                    .await
                    .unwrap();
            };

            rt.block_on(fut);
        })
    });
}

criterion_group!(
    benches,
    bench_ping,
    bench_prime_sieve_small,
    bench_prime_sieve_large,
    bench_echo_many
);
criterion_main!(benches);
