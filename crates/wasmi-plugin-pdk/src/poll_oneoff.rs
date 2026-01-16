#[link(wasm_import_module = "wasi_snapshot_preview1")]
unsafe extern "C" {
    fn poll_oneoff(subs: *const u8, events: *mut u8, nsubs: usize, nevents: *mut usize) -> u16;
}

/// A helper that uses WASI poll_oneoff to sleep the thread until STDIN is ready
pub fn wait_for_stdin() {
    let mut sub_bytes = [0u8; 48];
    let mut event_bytes = [0u8; 32];
    let mut nevents: usize = 0;

    sub_bytes[8] = 1;

    unsafe {
        poll_oneoff(
            sub_bytes.as_ptr(),
            event_bytes.as_mut_ptr(),
            1,
            &mut nevents,
        );
    }
}
