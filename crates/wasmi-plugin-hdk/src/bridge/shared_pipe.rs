//! Shared pipe implementation for worker bridge.

use std::io::{Read, Write};

use web_sys::{
    js_sys::{self, Int32Array, SharedArrayBuffer, Uint8Array},
    wasm_bindgen::JsCast,
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
        }
    }

    pub fn new_with_capacity(capacity: u32) -> Self {
        let sab = SharedArrayBuffer::new((DATA_OFFSET + capacity) as u32);
        Self::new(&sab)
    }

    fn is_closed(&self) -> bool {
        js_sys::Atomics::load(&self.header, CLOSED_IDX).unwrap() == 1
    }

    pub fn close(&self) {
        js_sys::Atomics::store(&self.header, CLOSED_IDX, 1).unwrap();
        // Notify both pointers to wake up anyone waiting on either end
        js_sys::Atomics::notify(&self.header, READ_IDX).unwrap();
        js_sys::Atomics::notify(&self.header, WRITE_IDX).unwrap();
    }

    fn read_idx(&self) -> u32 {
        js_sys::Atomics::load(&self.header, READ_IDX).unwrap() as u32
    }

    fn set_read(&self, value: u32) {
        js_sys::Atomics::store(&self.header, READ_IDX, value as i32).unwrap();
        js_sys::Atomics::notify(&self.header, READ_IDX).unwrap();
    }

    fn write_idx(&self) -> u32 {
        js_sys::Atomics::load(&self.header, WRITE_IDX).unwrap() as u32
    }

    fn set_write(&self, value: u32) {
        js_sys::Atomics::store(&self.header, WRITE_IDX, value as i32).unwrap();
        js_sys::Atomics::notify(&self.header, WRITE_IDX).unwrap();
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
