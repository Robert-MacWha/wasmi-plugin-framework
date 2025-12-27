#![cfg(target_arch = "wasm32")]

use gloo_timers::future::sleep;
use std::{
    io::{BufRead, BufReader, Write},
    sync::Once,
    time::Duration,
};
use tracing::info;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use wasmi_plugin_hdk::{
    runtime::non_blocking_pipe::non_blocking_pipe, worker_pool::worker_pool::WorkerPool,
};

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
async fn test_worker() {
    setup_logs();

    let (tx, rx) = futures::channel::oneshot::channel::<bool>();

    let task = async move || {
        info!("Hello from inside the worker!");
        tx.send(true).unwrap();
    };

    info!("Spawning worker...");
    let pool = WorkerPool::global();
    pool.run(task).expect("Failed to spawn worker");

    info!("Waiting for worker response...");
    let result = rx
        .await
        .expect("Worker dropped the sender without responding");

    assert!(result);
}

#[wasm_bindgen_test]
async fn test_worker_with_pipes() {
    setup_logs();

    let (stdin_reader, mut stdin_writer) = non_blocking_pipe();
    let (mut stdout_reader, stdout_writer) = non_blocking_pipe();

    let task = async move || {
        // Wait to recieve a message, then echo it back.
        let mut writer = stdout_writer;
        let mut reader = BufReader::new(stdin_reader);
        let mut line = String::new();
        loop {
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    // Echo back the line
                    writer.write_all(line.as_bytes()).unwrap();
                    writer.flush().unwrap();
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // Would block, try again
                    continue;
                }
                Err(e) => {
                    panic!("Error reading from stdin: {}", e);
                }
            }
        }
    };

    info!("Spawning worker...");
    let pool = WorkerPool::global();
    pool.run(task).expect("Failed to spawn worker");

    // Sleep for a sec to ensure the worker is ready
    sleep(Duration::from_millis(10)).await;

    let test_message = "Hello, worker!\n";
    info!("Sending message to worker: {}", test_message.trim());
    stdin_writer.write_all(test_message.as_bytes()).unwrap();
    stdin_writer.flush().unwrap();

    let mut reader = BufReader::new(&mut stdout_reader);
    let mut line = String::new();
    info!("Waiting for worker response...");
    loop {
        match reader.read_line(&mut line) {
            Ok(0) => panic!("Worker closed the pipe unexpectedly"),
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Would block, try again
                continue;
            }
            Err(e) => {
                panic!("Error reading from stdout: {}", e);
            }
        }
    }

    assert_eq!(line, test_message);
}
