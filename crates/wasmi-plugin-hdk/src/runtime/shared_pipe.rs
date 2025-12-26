//! Shared pipe implementation for worker bridge.

use std::{
    f64,
    io::{Read, Write},
    pin::Pin,
    task::{Context, Poll},
};

use futures::{AsyncRead, AsyncWrite};
use tracing::info;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    js_sys::{self, Atomics, Int32Array, SharedArrayBuffer, Uint8Array},
    wasm_bindgen::{JsCast, prelude::wasm_bindgen},
};

use crate::wasi::wasi_ctx::WasiReader;
// 4xi32 header: (16 bytes)
const CLOSED_IDX: u32 = 0; // 0 = open, 1 = closed
const WRITE_IDX: u32 = 1;
const READ_IDX: u32 = 2;
const DATA_OFFSET: u32 = 16;

// TODO: Impl AsyncRead so we don't need to busy-wait with WouldBlock
pub struct SharedPipe {
    header: Int32Array,
    data: Uint8Array,
    capacity: u32,

    name: String,
    pending_read: Option<JsFuture>,
    pending_write: Option<JsFuture>,
}

#[wasm_bindgen]
extern "C" {
    /// The object returned by Atomics.waitAsync
    pub type WaitAsyncResult;

    #[wasm_bindgen(method, getter, js_name = async)]
    pub fn is_async(this: &WaitAsyncResult) -> bool;

    #[wasm_bindgen(method, getter)]
    pub fn value(this: &WaitAsyncResult) -> wasm_bindgen::JsValue;
}

unsafe impl Send for SharedPipe {}
unsafe impl Sync for SharedPipe {}

impl SharedPipe {
    pub fn new(name: &str, sab: &SharedArrayBuffer) -> Self {
        let status = js_sys::Int32Array::new_with_byte_offset_and_length(&sab, 0, 4);
        let data_len = sab.byte_length() - DATA_OFFSET;
        let data = js_sys::Uint8Array::new_with_byte_offset_and_length(&sab, DATA_OFFSET, data_len);
        let capacity = data_len;

        Self {
            header: status,
            data,
            capacity,
            name: name.to_string(),
            pending_read: None,
            pending_write: None,
        }
    }

    pub fn new_with_capacity(name: &str, capacity: u32) -> Self {
        let sab = SharedArrayBuffer::new((DATA_OFFSET + capacity) as u32);
        Self::new(name, &sab)
    }

    pub fn reset(&self) {
        Atomics::store(&self.header, READ_IDX, 0).unwrap();
        Atomics::store(&self.header, WRITE_IDX, 0).unwrap();
        Atomics::store(&self.header, CLOSED_IDX, 0).unwrap();
    }

    pub fn close(&self) {
        Atomics::store(&self.header, CLOSED_IDX, 1).unwrap();
        // Notify both pointers to wake up anyone waiting on either end
        Atomics::notify(&self.header, READ_IDX).unwrap();
        Atomics::notify(&self.header, WRITE_IDX).unwrap();
    }

    pub fn buffer(&self) -> SharedArrayBuffer {
        self.header.buffer().unchecked_into::<SharedArrayBuffer>()
    }

    pub fn dump(&self) {
        //? Dumps internal state to logs for debugging
        let closed = self.is_closed();
        let r = self.read_idx();
        let w = self.write_idx();
        let occupied = self.get_occupied(r, w);

        let last_byte_count = occupied.min(50);
        let last_bytes = if occupied == 0 {
            vec![]
        } else {
            let mut bytes = vec![0u8; last_byte_count as usize];
            let start_idx = if w >= last_byte_count {
                w - last_byte_count
            } else {
                self.capacity + w - last_byte_count
            };
            for i in 0..last_byte_count {
                let idx = (start_idx + i) % self.capacity;
                bytes[i as usize] = self.data.get_index(idx);
            }
            bytes
        };

        // Interpret as UTF-8 if possible
        let last_bytes_str = match String::from_utf8(last_bytes.clone()) {
            Ok(s) => s,
            Err(_) => format!("{:?}", last_bytes),
        };

        info!(
            "SharedPipe[{}] dump: closed={} read_idx={} write_idx={} occupied={} last_bytes={}",
            self.name, closed, r, w, occupied, last_bytes_str
        );
    }

    fn is_closed(&self) -> bool {
        let closed = Atomics::load(&self.header, CLOSED_IDX).unwrap();
        closed == 1
    }

    fn read_idx(&self) -> u32 {
        Atomics::load(&self.header, READ_IDX).unwrap() as u32
    }

    fn set_read(&self, value: u32) {
        Atomics::store(&self.header, READ_IDX, value as i32).unwrap();
        Atomics::notify(&self.header, READ_IDX).unwrap();
    }

    fn write_idx(&self) -> u32 {
        Atomics::load(&self.header, WRITE_IDX).unwrap() as u32
    }

    fn set_write(&self, value: u32) {
        let old = Atomics::exchange(&self.header, WRITE_IDX, value as i32).unwrap();
        if old == value as i32 {
            panic!("set_write called with same value: {}", value);
        }
        Atomics::notify(&self.header, WRITE_IDX).unwrap();
    }

    fn get_occupied(&self, r: u32, w: u32) -> u32 {
        if w >= r { w - r } else { self.capacity - r + w }
    }

    fn try_read(&self, r: u32, w: u32, buf: &mut [u8]) -> Option<usize> {
        if r == w {
            if self.is_closed() {
                return Some(0); // EOF
            }
            return None;
        }

        let occupied = self.get_occupied(r, w);
        let amount = (buf.len() as u32).min(occupied);

        for i in 0..amount {
            let idx = (r + i) % self.capacity;
            buf[i as usize] = self.data.get_index(idx);
        }

        // info!(
        //     "SharedPipe[{}] try_read: r={} first_bytes={:?}",
        //     self.name,
        //     r,
        //     &buf[..(amount as usize).min(50)]
        // );
        Some(amount as usize)
    }

    fn try_write(&self, r: u32, w: u32, buf: &[u8]) -> Option<usize> {
        if self.is_closed() {
            return None;
        }

        let occupied = self.get_occupied(r, w);
        //? The buffer is full if occupied == capacity - 1
        let available = self.capacity.saturating_sub(occupied).saturating_sub(1);

        if available == 0 {
            return None;
        }

        let n = (buf.len() as u32).min(available);

        for i in 0..n {
            let idx = (w + i) % self.capacity;
            self.data.set_index(idx, buf[i as usize]);
        }

        // info!(
        //     "SharedPipe[{}] try_write: w={} first_bytes={:?}",
        //     self.name,
        //     w,
        //     &buf[..buf.len().min(50)]
        // );
        Some(n as usize)
    }
}

impl Read for SharedPipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let r = self.read_idx();
        let w = self.write_idx();

        if let Some(n) = self.try_read(r, w, buf) {
            self.set_read((r + n as u32) % self.capacity);
            return Ok(n);
        }

        Err(std::io::ErrorKind::WouldBlock.into())
    }
}

impl Write for SharedPipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let r = self.read_idx();
        let w = self.write_idx();

        if let Some(n) = self.try_write(r, w, buf) {
            self.set_write((w + n as u32) % self.capacity);
            return Ok(n);
        }

        Err(std::io::ErrorKind::WouldBlock.into())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl WasiReader for SharedPipe {
    fn is_ready(&self) -> bool {
        let r = self.read_idx();
        let w = self.write_idx();

        r != w || self.is_closed()
    }

    fn wait_ready(&self, timeout: Option<web_time::Duration>) {
        let timeout_ms = timeout
            .map(|d| d.as_millis() as f64)
            .unwrap_or(f64::INFINITY);

        loop {
            let (r, w) = (self.read_idx(), self.write_idx());

            if r != w || self.is_closed() {
                return;
            }

            match Atomics::wait_with_timeout(&self.header, WRITE_IDX, w as i32, timeout_ms) {
                Ok(s) if s == "not-equal" => {
                    //? WRITE_IDX changed, re-check
                    continue;
                }
                _ => {
                    //? Resolved
                    return;
                }
            }
        }
    }
}

impl AsyncRead for SharedPipe {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        //? Check for existing promise
        if let Some(mut fut) = self.pending_read.take() {
            if Pin::new(&mut fut).poll(cx).is_pending() {
                self.pending_read = Some(fut);
                return Poll::Pending;
            }
        }

        //? Try sync read
        loop {
            let (r, w) = (self.read_idx(), self.write_idx());

            if let Some(n) = self.try_read(r, w, buf) {
                self.set_read((r + n as u32) % self.capacity);
                return Poll::Ready(Ok(n));
            }

            //? Wait for WRITE_IDX to change
            let res: WaitAsyncResult = Atomics::wait_async(&self.header, WRITE_IDX, w as i32)
                .expect("waitAsync support")
                .unchecked_into();

            if !res.is_async() {
                //? WRITE_IDX changed, re-check
                continue;
            }

            let current_w = self.write_idx();
            if current_w != w {
                continue;
            }

            let promise: js_sys::Promise = res.value().unchecked_into();
            let mut fut = JsFuture::from(promise);

            if let Poll::Ready(_) = Pin::new(&mut fut).poll(cx) {
                //? Resolved immediately, re-check
                continue;
            }

            self.pending_read = Some(fut);
            return Poll::Pending;
        }
    }
}

impl AsyncWrite for SharedPipe {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        //? Drain existing promise
        if let Some(mut fut) = self.pending_write.take() {
            if Pin::new(&mut fut).poll(cx).is_pending() {
                self.pending_write = Some(fut);
                return Poll::Pending;
            }
        }

        loop {
            let (r, w) = (self.read_idx(), self.write_idx());

            if let Some(n) = self.try_write(r, w, buf) {
                self.set_write((w + n as u32) % self.capacity);
                return Poll::Ready(Ok(n));
            }

            //? Wait for READ_IDX to change
            let res: WaitAsyncResult = Atomics::wait_async(&self.header, READ_IDX, r as i32)
                .expect("waitAsync support required")
                .unchecked_into();

            if !res.is_async() {
                //? READ_IDX changed, re-check
                continue;
            }

            let promise = res.value().unchecked_into::<js_sys::Promise>();
            let mut fut = JsFuture::from(promise);

            if let Poll::Ready(_) = Pin::new(&mut fut).poll(cx) {
                //? Resolved immediately, re-check
                continue;
            }

            self.pending_write = Some(fut);
            return Poll::Pending;
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.close();
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {

    #[cfg(target_family = "wasm")]
    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn test_basic_read_write() {
        let mut pipe = SharedPipe::new_with_capacity("test", 10);

        let input = b"hello";
        let written = pipe.write(input).expect("Write failed");
        assert_eq!(written, 5);

        let mut output = [0u8; 5];
        let read = pipe.read(&mut output).expect("Read failed");
        assert_eq!(read, 5);
        assert_eq!(&output, b"hello");

        // Should now be WouldBlock
        let res = pipe.read(&mut output);
        assert!(res.is_err());

        pipe.close();
        let res = pipe.write(b"more");
        assert!(res.is_err());

        let res = pipe.read(&mut output);
        assert_eq!(res.ok(), Some(0)); // EOF
    }

    #[wasm_bindgen_test]
    fn test_wrap_around_saturation() {
        let mut pipe = SharedPipe::new_with_capacity("test", 10);

        // 1. Fill 7 bytes
        pipe.write(b"hello_w").unwrap();

        // 2. Read 4 bytes (read_idx moves to 4, write_idx is at 7)
        let mut tmp = [0u8; 4];
        pipe.read(&mut tmp).unwrap();
        assert_eq!(&tmp, b"hell");

        // 3. Write 5 more bytes.
        // Current state: r=4, w=7. Space available: 10 - (7-4) - 1 = 6.
        // This write will wrap! (7, 8, 9, then 0, 1)
        let written = pipe.write(b"orld_").unwrap();
        assert_eq!(written, 5);

        // 4. Read all available data (should be "efg" + "hijkl" = 8 bytes)
        // This read MUST saturate the output buffer across the wrap point.
        let mut output = [0u8; 8];
        let read = pipe.read(&mut output).expect("Wrap read failed");

        assert_eq!(read, 8);
        assert_eq!(&output, b"o_world_");

        // Verify pointers are back in sync
        assert_eq!(pipe.read_idx(), pipe.write_idx());
    }
}
