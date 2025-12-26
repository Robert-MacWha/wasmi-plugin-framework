function getTimestamp() {
    const now = new Date();

    const YYYY = now.getFullYear();
    const MM = String(now.getMonth() + 1).padStart(2, '0');
    const DD = String(now.getDate()).padStart(2, '0');
    const hh = String(now.getHours()).padStart(2, '0');
    const mm = String(now.getMinutes()).padStart(2, '0');
    const ss = String(now.getSeconds()).padStart(2, '0');
    const ms = String(now.getMilliseconds()).padStart(3, '0');

    return `${YYYY}-${MM}-${DD} ${hh}:${mm}:${ss}.${ms}`;
}

console.log = function (...args) {
    const prefix = `[${getTimestamp()}]`;
    self.postMessage({ type: 'Log', message: `${prefix} ${args.join(' ')}` });
};

console.error = function (...args) {
    const prefix = `[${getTimestamp()}] [ERROR]`;
    self.postMessage({ type: 'Log', message: `${prefix} ${args.join(' ')}` });
};

console.log("Worker Shim: Script starting...");

{ WORKER_JS_GLUE }

wasm_bindgen('{ WASM_URL }').then(() => {
    console.log("Worker Shim: Rust Wasm Initialized");
    wasm_bindgen.start_worker();
}).catch(err => console.error("Worker Init Failed:", err));