#[cfg(not(target_arch = "wasm32"))]
mod benchmarks {
    use criterion::{Criterion, criterion_group};
    use serde_json::Value;
    use std::{
        sync::{Arc, Once},
        time::Duration,
    };
    use tokio::runtime::Builder;
    use wasmi_plugin_hdk::{plugin::Plugin, plugin_id::PluginId, server::HostServer};
    use wasmi_plugin_pdk::transport::AsyncTransport;

    const PLUGIN_WASM: &[u8] =
        include_bytes!("../../../target/wasm32-wasip1/release/test-plugin-async.wasm");

    static INIT: Once = Once::new();

    fn load_plugin_wasm() -> Vec<u8> {
        PLUGIN_WASM.to_vec()
    }

    async fn load_plugin() -> Plugin {
        let wasm_bytes = load_plugin_wasm();
        let handler = Arc::new(get_host_server());
        let plugin = Plugin::builder("test_plugin", wasm_bytes.clone(), handler)
            .with_timeout(Duration::from_secs(10))
            .build()
            .await
            .unwrap();

        plugin
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
            // tracing_subscriber::fmt()
            //     .compact()
            //     .with_ansi(false)
            //     .with_max_level(tracing_subscriber::filter::LevelFilter::INFO)
            //     .with_test_writer()
            //     .init();
        });
    }

    /// Benchmark a single ping request to the wasm module.
    pub fn bench_ping(c: &mut Criterion) {
        setup_logs();

        let rt = Builder::new_current_thread().enable_all().build().unwrap();
        let plugin = rt.block_on(load_plugin());

        c.bench_function("ping", |b| {
            b.iter(|| {
                let fut = async {
                    plugin.call_async("ping", Value::Null).await.unwrap();
                };

                rt.block_on(fut);
            })
        });
    }

    /// Benchmark the full lifecycle of creating a plugin instance and calling a ping request.
    /// Tests the overhead of plugin instantiation.
    pub fn bench_lifecycle(c: &mut Criterion) {
        setup_logs();

        let rt = Builder::new_current_thread().enable_all().build().unwrap();

        let wasm_bytes = load_plugin_wasm();
        let handler = Arc::new(get_host_server());

        c.bench_function("lifecycle", |b| {
            b.iter(|| {
                let fut = async {
                    let plugin =
                        Plugin::builder("test_plugin", wasm_bytes.clone(), handler.clone())
                            .with_timeout(Duration::from_secs(10))
                            .build()
                            .await
                            .unwrap();
                    plugin.call_async("ping", Value::Null).await.unwrap();
                };

                rt.block_on(fut);
            })
        });
    }

    /// Benchmark the prime sieve function with a small input. Primarily tests the overhead
    /// of calling into the wasm module.
    pub fn bench_prime_sieve_small(c: &mut Criterion) {
        setup_logs();

        let rt = Builder::new_current_thread().enable_all().build().unwrap();
        let plugin = rt.block_on(load_plugin());

        c.bench_function("prime_sieve_small", |b| {
            b.iter(|| {
                let fut = async {
                    plugin
                        .call_async("prime_sieve", Value::Number(1.into()))
                        .await
                        .unwrap();
                };

                rt.block_on(fut);
            })
        });
    }

    /// Benchmark the prime sieve function with a large input. Tests performance within the wasm module.
    pub fn bench_prime_sieve_large(c: &mut Criterion) {
        setup_logs();

        let rt = Builder::new_current_thread().enable_all().build().unwrap();
        let plugin = rt.block_on(load_plugin());

        c.bench_function("prime_sieve_large", |b| {
            b.iter(|| {
                let fut = async {
                    plugin
                        .call_async("prime_sieve", Value::Number(1_000_000.into()))
                        .await
                        .unwrap();
                };

                rt.block_on(fut);
            })
        });
    }

    /// Benchmark sending many echo requests to the host, and receiving responses.
    pub fn bench_call_many(c: &mut Criterion) {
        setup_logs();

        let rt = Builder::new_current_thread().enable_all().build().unwrap();
        let plugin = rt.block_on(load_plugin());

        c.bench_function("call_many", |b| {
            b.iter(|| {
                let fut = async {
                    plugin
                        .call_async("call_many", Value::Number(200.into()))
                        .await
                        .unwrap();
                };

                rt.block_on(fut);
            })
        });
    }

    /// Benchmark sending many echo requests to the host, and receiving responses.
    pub fn bench_call_many_async(c: &mut Criterion) {
        setup_logs();

        let rt = Builder::new_current_thread().enable_all().build().unwrap();
        let plugin = rt.block_on(load_plugin());

        c.bench_function("call_many_async", |b| {
            b.iter(|| {
                let fut = async {
                    plugin
                        .call_async("call_many_async", Value::Number(200.into()))
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
        bench_lifecycle,
        bench_prime_sieve_small,
        bench_prime_sieve_large,
        bench_call_many,
        bench_call_many_async
    );
}

#[cfg(not(target_arch = "wasm32"))]
criterion::criterion_main!(benchmarks::benches);

#[cfg(target_arch = "wasm32")]
fn main() {}
