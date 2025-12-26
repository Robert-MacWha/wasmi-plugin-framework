//! Web Worker runtime bridge, running the Wasm module in a Web Worker.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use futures::lock::Mutex;
use futures::{AsyncRead, AsyncWrite};

use crate::compile::Compiled;
use crate::runtime::Runtime;
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
        let (session, stdin, stdout) = get_pool().lock().await.run(compiled).await?;

        let instance_id = session.id;
        self.active_sessions
            .lock()
            .await
            .insert(instance_id, session);

        Ok((instance_id as u64, stdin, stdout))
    }

    async fn terminate(self, instance_id: u64) {
        let instance_id = instance_id as WorkerId;
        if let Some(session) = self.active_sessions.lock().await.remove(&instance_id) {
            session.terminate();
        }
    }
}
