//! Shared pipe implementation for worker bridge.

use std::{
    io::{Read, Write},
    pin::Pin,
    task::{Context, Poll},
};

use futures::{AsyncRead, AsyncWrite};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    js_sys::{self, Atomics, Int32Array, SharedArrayBuffer, Uint8Array},
    wasm_bindgen::{JsCast, prelude::wasm_bindgen},
};

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
    pub fn new(sab: &SharedArrayBuffer) -> Self {
        let status = js_sys::Int32Array::new_with_byte_offset_and_length(&sab, 0, 4);
        let data_len = sab.byte_length() - DATA_OFFSET;
        let data = js_sys::Uint8Array::new_with_byte_offset_and_length(&sab, DATA_OFFSET, data_len);
        let capacity = data_len;

        Self {
            header: status,
            data,
            capacity,
            pending_read: None,
            pending_write: None,
        }
    }

    pub fn new_with_capacity(capacity: u32) -> Self {
        let sab = SharedArrayBuffer::new((DATA_OFFSET + capacity) as u32);
        Self::new(&sab)
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

    fn is_closed(&self) -> bool {
        Atomics::load(&self.header, CLOSED_IDX).unwrap() == 1
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
        Atomics::store(&self.header, WRITE_IDX, value as i32).unwrap();
        Atomics::notify(&self.header, WRITE_IDX).unwrap();
    }

    pub fn buffer(&self) -> SharedArrayBuffer {
        self.header.buffer().unchecked_into::<SharedArrayBuffer>()
    }
}

impl Read for SharedPipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let r = self.read_idx();
        let w = self.write_idx();

        if r == w {
            //? Only check if closed when empty, so we can drain remaining data
            // after close
            if self.is_closed() {
                return Ok(0);
            }
            return Err(std::io::ErrorKind::WouldBlock.into());
        }

        let buf_len = buf.len() as u32;
        let occupied = if w >= r { w - r } else { self.capacity - r + w };
        let amount = occupied.min(buf_len);

        // Chunk 1: From R to (End of Buffer or W)
        let first_chunk_len = if w >= r {
            amount
        } else {
            (self.capacity - r).min(amount)
        };
        self.data
            .subarray(r, r + first_chunk_len)
            .copy_to(&mut buf[..first_chunk_len as usize]);

        // Chunk 2: From 0 to remaining (only if we wrapped)
        if amount > first_chunk_len {
            let second_chunk_len = amount - first_chunk_len;
            self.data
                .subarray(0, second_chunk_len)
                .copy_to(&mut buf[first_chunk_len as usize..amount as usize]);
        }

        self.set_read((r + amount) % self.capacity);
        Ok(amount as usize)
    }
}

impl Write for SharedPipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.is_closed() {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }

        let r = self.read_idx();
        let w = self.write_idx();

        let occupied = if w >= r { w - r } else { self.capacity - r + w };
        let available = self.capacity - occupied - 1; // N-1 rule

        if available == 0 {
            return Err(std::io::ErrorKind::WouldBlock.into());
        }

        let n = (buf.len() as u32).min(available);

        // Chunk 1: From W to (End of Buffer or R-1)
        let first_chunk_len = if r > w { n } else { (self.capacity - w).min(n) };
        self.data.set(
            &js_sys::Uint8Array::from(&buf[..first_chunk_len as usize]),
            w,
        );

        // Chunk 2: From 0 to remaining (only if we wrap)
        if n > first_chunk_len {
            let second_chunk_len = n - first_chunk_len;
            let src = &buf[first_chunk_len as usize..(first_chunk_len + second_chunk_len) as usize];
            self.data.set(&js_sys::Uint8Array::from(src), 0);
        }

        self.set_write((w + n) % self.capacity);
        Ok(n as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
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
        match self.read(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                //? Setup async wait for WRITE_IDX to change
                let current_w = self.write_idx() as i32;
                let res: WaitAsyncResult = Atomics::wait_async(&self.header, WRITE_IDX, current_w)
                    .expect("Atomics.waitAsync should be supported")
                    .unchecked_into();

                if !res.is_async() {
                    // res was "not-equal", meaning another thread changed WRITE_IDX
                    // already. So wake the caller.
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }

                let promise: js_sys::Promise = res.value().unchecked_into();
                let mut fut = JsFuture::from(promise);

                if Pin::new(&mut fut).poll(cx).is_pending() {
                    self.pending_read = Some(fut);
                    return Poll::Pending;
                }

                // Future resolved, meaning WRITE_IDX changed, try read again
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl AsyncWrite for SharedPipe {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if let Some(mut fut) = self.pending_write.take() {
            if Pin::new(&mut fut).poll(cx).is_pending() {
                self.pending_write = Some(fut);
                return Poll::Pending;
            }
        }

        match self.write(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Writer waits for READ_IDX to change (meaning space freed up)
                let current_r = self.read_idx() as i32;
                let res: WaitAsyncResult = Atomics::wait_async(&self.header, READ_IDX, current_r)
                    .expect("waitAsync support required")
                    .unchecked_into();

                if !res.is_async() {
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }

                let mut fut = JsFuture::from(res.value().unchecked_into::<js_sys::Promise>());
                if Pin::new(&mut fut).poll(cx).is_pending() {
                    self.pending_write = Some(fut);
                    return Poll::Pending;
                }

                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
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
        let mut pipe = SharedPipe::new_with_capacity(10);

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
        let mut pipe = SharedPipe::new_with_capacity(10);

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
