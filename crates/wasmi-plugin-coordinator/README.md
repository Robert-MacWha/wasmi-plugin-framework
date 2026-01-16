

## Definitions
- Main Thread: The main JavaScript thread that runs the UI.
- Coordinator: Compute Worker coordinator that manages multiple compute workers, dispatching tasks and acting as a bridge between the main thread and compute workers.
- Compute Worker: Web workers that can run blocking computations without freezing the UI.

## Notes for the future Robert:
1. Use `worker.post_message` and `set_onmessage` to communicate between the main thread and compute workers directly:

Can't do this because `wasmer`'s `function::call` function is sync and blocking. That's fine for nearly everything except `fd_read`. For `fd_read` we need to recieve a message from the thread running the `host` so we can either fetch data or call another plugin or whatnot.  However, since receiving messages is handled via the `set_onmessage` callback, we would need to yield to the JS event loop for it to tick.  Since we can't yield from a sync function, we can't receive messages while in a blocking call, and we can't implement `fd_read`.

1.1. Use JSPI to fix this

NOTE: Based on the asyncify tests, this may not be feisible anyways due to performance. No clue whether JSPI would be faster or slower than asyncify, but because
it's doing the same stuff it'd probably be order-of-magnitude equal.

Good try, but no luck.  While [JSPI](https://www.dgendill.com/posts/technology/2025-10-01-web-assembly-javascript-promise-integration.html) would allow us to forcefully yield to the JS event loop from within a sync function, it isn't supported by `wasm-bindgen` yet. You may think we could use raw linking to call JSPI directly, but we can't:
 - `wasmer` requires certain `wasm-bindgen` imports, meaning we need to include those when spawning our worker.
 - `wasm-bindgen` does not support JSPI imports yet.
 - `wasm-bindgen` also doesn't support adding custom raw imports *at all*.
 - And finally, you can't wrap a `WebAssembly.Suspending` function around or within a `wasm-bindgen` function because
   - `wasm-bindgen` can't handle `WebAssembly.Suspending` functions if you try to import them directly.
   - `WebAssembly.Suspending` functions can't be called from nested functions (IE if you wrapped it in a `wasm-bindgen` function). The function imported into the wasm MUST BE the `WebAssembly.Suspending` instance itself, not a wrapper that constructs or creates one.
   - You could *possibly* manually inject the suspending function into wasm-bindgen's imports manually using internal functions, but that would be very very fragile.

Tracking issue: https://github.com/wasm-bindgen/wasm-bindgen/issues/3633

2. Use SharedArrayBuffers for shared memory between the main thread and compute workers, without needing full atomics or a coordinator

I tried this for literally 20 hours and could never fully get rid of race conditions.  I fully believe it's possibly, I just also fully believe that I'm not a clever enough programmer / familiar enough with web-based concurrency to pull it off.

The main issue was that I could never get the read / write IDXs for the ring buffers to be perfectly in sync between the main and compute worker thread.  Inevitably something would always get out of sync and both threads would deadlock waiting for a message from the other.

3. Use atomics + shared memory in the main thread directly to communicate with compute workers.

This nearly works if it weren't for the UI.  The main thread is perfectly capable of using shared memory to talk with web workers and, by passing raw function ptrs between threads, you can get an interface that basically feels like std::thread::spawn.  However, the main thread is also responsible for running UI and no matter how much I've tried I can't get dioxus compiling with atomics + shared memory enabled.

Tracking issue: https://github.com/DioxusLabs/dioxus/issues/5079

4. Asyncify the wasi to alloy yielding to the caller when required, who can then yield to the event loop as needed.

https://kripken.github.io/blog/wasm/2019/07/16/asyncify.html

Asyncify SHOULD work, it's just too slow.  For anything that does a lot of host calls, the overhead of unwinding / rewinding the stack is too high.  Below are some benchmark results from a test I performed where I:
 - Added asyncify.
 - For all `fd_read` calls, if they would block, unwind the stack and return to the host.

Otherwise it was running in the same shared memory environment as before. stdio used the NonBlockingPipe, not worker messages. This was purely measuring the overhead of asyncify + unwinding / rewinding + yielding to JS on `fd_read` calls.

```bash
# Runs 200 `call` or `call_async` calls
Collecting 100 samples in estimated 5.2375 s (2000 iterations)
call_many_async         time:   [2.5450 ms 2.6233 ms 2.7039 ms]

Collecting 100 samples in estimated 82.924 s (100 iterations)
call_many               time:   [819.96 ms 824.37 ms 828.77 ms]
```

It's pretty abysmal. Enough said. In the real world `call_many_async` may stay lower (will still be at least an order of magnitude slower). But `call_many` will only get worse since it needs to wait for the messages.  Best-case it would be ~800ms, more likely ~2s.

Other downsides I documented before testing things:
  - Post-processing the wasm with the asyncify pass
  - Causes a slowdown due to extra stack management & state machinery
  - Requires wasm32-unknown-unknown-specific code in wasi_ctx to setup and call asyncify.
  - Likely significant slowdown. 
    - The asyncify stack unwinding will be slow, even if we optimize it to only unwind on `fd_read` when required.
    - Because unwinding / rewinding is slow, we probably won't be able to do it very often. So the idea of constantly running a background `read` task in the transport driver that enables concurrent async IO probably won't be feasible since it would require rewinding on every `fd_read` call.

5. Use a seperate coordinator worker to manage compute workers and communicate with the main thread.

Essentially a workarround for 3's issues. By having a coordinator worker that communicates with the compute workers using shared memory and with the main thread using postMessage, we can avoid the need for atomics in the main thread.  The coordinator can manage all the compute instances and dispatch tasks to them via function pointers, and then relay messages back to the main thread.

The primary issue here is software architecture. Deciding how to split responsibilities between the main thread and coordinator.
  - If `host` is entirely in the coordinator, then the main thread must make a postMessage call for every host interaction, even those that don't involve compute workers. This probably won't be too slow, but will be annoying to setup (wasm-specific proxy that'll pass calls back and forth).
  - If `host` is in the main thread, and only sends requests to the coordinator when compute workers are involved, then the coordinator will need to query the host whenever any computer worker needs to interact with the host.

I'm going to go with the latter approach since it seems cleaner. There is some significant added latency in messages vs shared memory for inter-plugin communication, but it also results in a much much much simpler architecture and UX for consumers of the library.
  - Namely - with the former approach, it would require placing consumer code in the coordinator worker.  That means consumers of the library would need to write and compile code for the coordinator, which means setting up rust nightly, atomics + shared memory, and having a seperate crate.  With the latter I can just bundle the compiled generic `.wasm` coordinator with the library for use as-is.