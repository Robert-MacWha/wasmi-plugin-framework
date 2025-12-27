use std::io::Write;

use web_sys::{console::log_1, js_sys};

pub struct MessageWriter {
    name: String,
}

unsafe impl Send for MessageWriter {}
unsafe impl Sync for MessageWriter {}

impl MessageWriter {
    #[allow(dead_code)]
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

impl Write for MessageWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut prefix = format!("[{}] ", self.name).into_bytes();
        prefix.extend_from_slice(buf);
        let prefix = prefix.as_slice();

        log_1(js_sys::Uint8Array::from(prefix).as_ref());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
