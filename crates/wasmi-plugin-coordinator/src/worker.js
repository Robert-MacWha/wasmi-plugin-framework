// console.log = function (...args) {
//     self.postMessage({ type: 'Log', message: `${args.join(' ')}` });
// };

// console.error = function (...args) {
//     self.postMessage({ type: 'Log', message: `${args.join(' ')}` });
// };

import init, { start_coordinator } from "{sdk_url}";

init('{wasm_url}').then(() => {
    start_coordinator();
}).catch((err) => {
    console.error("Worker Init Failed:", err);
});
