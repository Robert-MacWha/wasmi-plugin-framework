pub fn yield_now() -> impl std::future::Future<Output = ()> {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        gloo_timers::future::TimeoutFuture::new(0)
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        tokio::task::yield_now()
    }
}
