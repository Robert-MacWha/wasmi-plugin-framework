// console.log = function (...args) {
//     self.postMessage({ type: 'Log', message: `${args.join(' ')}` });
// };

// console.error = function (...args) {
//     self.postMessage({ type: 'Log', message: `${args.join(' ')}` });
// };

console.log("Worker JS initialized");

import init, { start_coordinator } from "{sdk_url}";

console.log("JS Glue loaded, initializing wasm...");

init('{wasm_url}').then(() => {
    start_coordinator();
    console.log("Wasm module loaded, starting worker...");
}).catch((err) => {
    console.error("Worker Init Failed:", err);
});
