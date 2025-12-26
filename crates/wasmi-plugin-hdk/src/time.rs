pub async fn sleep(dur: web_time::Duration) {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        gloo_timers::future::TimeoutFuture::new(dur.as_millis() as u32).await;
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        tokio::time::sleep(dur).await;
    }
}

pub fn blocking_sleep(dur: web_time::Duration) {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        let stn = web_sys::js_sys::SharedArrayBuffer::new(4);
        let typed_array = web_sys::js_sys::Int32Array::new(&stn);

        let millis = dur.as_millis() as f64;
        let _ = web_sys::js_sys::Atomics::wait_with_timeout(&typed_array, 0, 0, millis);
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        std::thread::sleep(dur);
    }
}
