//! Web Worker runtime bridge, running the Wasm module in a Web Worker.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use futures::io::BufReader;
use futures::lock::Mutex;
use futures::{AsyncBufReadExt, AsyncRead, AsyncWrite};
use tracing::{error, info};
use wasm_bindgen_futures::spawn_local;
use wasmi_plugin_rt::yield_now;

use crate::compile::Compiled;
use crate::runtime::Runtime;
use crate::runtime::shared_pipe::SharedPipe;
use crate::runtime::worker_pool::{PoolError, WorkerId, WorkerPool, WorkerSession};

static POOL: OnceLock<Mutex<WorkerPool>> = OnceLock::new();

pub fn get_pool() -> &'static Mutex<WorkerPool> {
    POOL.get_or_init(|| Mutex::new(WorkerPool::new().expect("Failed to init global worker pool")))
}

pub struct WorkerRuntime {
    active_sessions: Arc<Mutex<HashMap<WorkerId, WorkerSession>>>,
}

impl WorkerRuntime {
    pub fn new() -> Self {
        WorkerRuntime {
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for WorkerRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime for WorkerRuntime {
    type Error = PoolError;

    async fn spawn(
        &self,
        compiled: Compiled,
    ) -> Result<
        (
            u64,
            impl AsyncWrite + Unpin + Send + Sync + 'static,
            impl AsyncRead + Unpin + Send + Sync + 'static,
        ),
        Self::Error,
    > {
        let name = compiled.name.clone();
        let (session, stdin, stdout, stderr) = get_pool().lock().await.run(compiled).await?;

        let instance_id = session.id;
        self.active_sessions
            .lock()
            .await
            .insert(instance_id, session);

        Self::spawn_stderr(name, stderr);

        Ok((instance_id as u64, stdin, stdout))
    }

    async fn terminate(self, instance_id: u64) {
        let instance_id = instance_id as WorkerId;
        if let Some(session) = self.active_sessions.lock().await.remove(&instance_id) {
            session.terminate();
        }
    }
}

impl WorkerRuntime {
    fn spawn_stderr(name: String, mut stderr_reader: SharedPipe) {
        spawn_local(async move {
            let mut stderr = BufReader::new(&mut stderr_reader);
            let mut buffer = String::new();
            loop {
                buffer.clear();
                match stderr.read_line(&mut buffer).await {
                    Ok(0) => {
                        break;
                    }
                    Ok(_) => {
                        info!("[plugin] [{}] {}", name, buffer.trim_end());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        yield_now().await;
                        continue;
                    }
                    Err(e) => {
                        error!("[WorkerBridge] Error reading from stderr: {}", e);
                        break;
                    }
                }
            }
        });
    }
}
