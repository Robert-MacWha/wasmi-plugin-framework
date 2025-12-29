

## Definitions
- Main Thread: The main JavaScript thread that runs the UI.
- Coordinator: Compute Worker coordinator that manages multiple compute workers, dispatching tasks and acting as a bridge between the main thread and compute workers.
- Compute Worker: Web workers that can run blocking computations without freezing the UI.

## Notes for the future Robert:
1. Use `worker.post_message` and `set_onmessage` to communicate between the main thread and compute workers directly:

Can't do this because `wasmer`'s `function::call` function is sync and blocking. That's fine for nearly everything except `fd_read`. For `fd_read` we need to recieve a message from the thread running the `host` so we can either fetch data or call another plugin or whatnot.  However, since receiving messages is handled via the `set_onmessage` callback, we would need to yield to the JS event loop for it to tick.  Since we can't yield from a sync function, we can't receive messages while in a blocking call, and we can't implement `fd_read`.

1.1. Use JSPI to fix this

Good try, but no luck.  While [JSPI](https://www.dgendill.com/posts/technology/2025-10-01-web-assembly-javascript-promise-integration.html) would allow us to forcefully yield to the JS event loop from within a sync function, it isn't supported by `wasm-bindgen` yet. You may think we could use raw linking to call JSPI directly, but we can't:
 - `wasmer` requires certain `wasm-bindgen` imports, meaning we need to include those when spawning our worker.
 - `wasm-bindgen` does not support JSPI imports yet.
 - `wasm-bindgen` also doesn't support adding custom raw imports *at all*.
 - And finally, you can't wrap a `WebAssembly.Suspending` function around or within a `wasm-bindgen` function because
   - `wasm-bindgen` can't handle `WebAssembly.Suspending` functions if you try to import them directly.
   - `WebAssembly.Suspending` functions can't be called from nested functions (IE if you wrapped it in a `wasm-bindgen` function). The function imported into the wasm MUST BE the `WebAssembly.Suspending` instance itself, not a wrapper that constructs or creates one.

1. Use SharedArrayBuffers for shared memory between the main thread and compute workers, without needing full atomics or a coordinator
