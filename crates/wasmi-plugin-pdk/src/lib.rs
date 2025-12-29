pub mod api;
mod poll_oneoff;
pub mod router;
pub mod rpc_message;
pub mod runner;
pub mod transport;

const BUFFER_LEN: usize = 8200;

#[unsafe(no_mangle)]
static mut ASYNCIFY_BUFFER: [u8; BUFFER_LEN] = [0; BUFFER_LEN];

#[unsafe(no_mangle)]
pub extern "C" fn get_asyncify_ptr() -> *mut u8 {
    std::ptr::addr_of_mut!(ASYNCIFY_BUFFER) as *mut u8
}

#[unsafe(no_mangle)]
pub extern "C" fn get_asyncify_len() -> u32 {
    BUFFER_LEN as u32
}
