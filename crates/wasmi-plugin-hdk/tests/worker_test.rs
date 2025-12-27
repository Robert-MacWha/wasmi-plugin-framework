use std::sync::Once;
use tracing::info;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use wasmi_plugin_hdk::runtime::worker_pool::{WorkerHandle, WorkerPool};

wasm_bindgen_test_configure!(run_in_browser);

static INIT: Once = Once::new();

fn setup_logs() {
    INIT.call_once(|| {
        tracing_wasm::set_as_global_default_with_config(
            tracing_wasm::WASMLayerConfigBuilder::new()
                .set_console_config(tracing_wasm::ConsoleConfig::ReportWithoutConsoleColor)
                .build(),
        );
    });
}

#[wasm_bindgen_test]
async fn test_worker_with_channel() {
    setup_logs();

    let (tx, rx) = futures::channel::oneshot::channel::<bool>();

    let task = move || {
        info!("Worker started");
        tx.send(true).unwrap();
    };

    info!("Spawning worker...");
    let pool = WorkerPool::global();
    pool.run(task).await.expect("Failed to spawn worker");

    info!("Waiting for worker response...");
    let result = rx
        .await
        .expect("Worker dropped the sender without responding");

    assert!(result);
}
