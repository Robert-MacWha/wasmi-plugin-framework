// Forwards JS and WASM logs to the main thread
// Very slow, so disabled unless required for debugging
["log", "info", "warn", "error"].forEach(level => {
    globalThis.console[level] = (...args) => {
        globalThis.postMessage({
            type: "Log",
            level: level,
            message: args.map(String).join(" "),
            ts: globalThis.performance.now()
        });
    };
});

console.log("Worker started");
let wasmPkg = null;
let processingQueue = Promise.resolve();

globalThis.onmessage = async ev => {
    processingQueue = processingQueue.then(async () => {
        try {
            if (ev.data.type === "init") {
                const { memory, module, sdkUrl } = ev.data;

                wasmPkg = await import(sdkUrl);
                const init = wasmPkg.default || wasmPkg.init;

                if (typeof init !== 'function') {
                    throw new Error("Wasm init is not a function");
                }

                await init(module, memory);
            } else if (ev.data.type === "run") {
                const { taskPtr } = ev.data;
                try {
                    await wasmPkg.execute_worker_task(taskPtr);
                } finally {
                    globalThis.postMessage({ type: "Idle" });
                }
            } else if (ev.data.type === "run_with") {
                const { taskPtr, extra } = ev.data;
                console.log("(JS) extra:", extra, typeof extra);
                try {
                    await wasmPkg.execute_worker_task_with(taskPtr, extra);
                } finally {
                    globalThis.postMessage({ type: "Idle" });
                }
            } else {
                console.error(`Unknown message type: ${type}`);
            }
        } catch (e) {
            console.error(`Error in worker: ${e.message}`);
        }
    });
};