console.log = function (...args) {
    self.postMessage({ type: 'Log', message: args.join(' ') });
};
console.error = function (...args) {
    self.postMessage({ type: 'Log', message: args.join(' ') });
};

console.log("Worker Shim: Script starting...");

{ WORKER_JS_GLUE }

wasm_bindgen('{ WASM_URL }').then(() => {
    console.log("Worker Shim: Rust Wasm Initialized");
    wasm_bindgen.start_worker();
}).catch(err => console.error("Worker Init Failed:", err));