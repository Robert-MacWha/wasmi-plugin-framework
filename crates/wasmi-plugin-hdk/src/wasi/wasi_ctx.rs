use std::io::{BufRead, Read, Write};
use thiserror::Error;
use tracing::{error, info, trace, warn};
use wasmer::{FunctionEnvMut, RuntimeError};
use web_time::{Duration, Instant, SystemTime};

use crate::time::blocking_sleep;

/// A WASI context that can be attached to a wasmi instance. Attaches
/// a subset of WASI syscalls to the instance, allowing it to
/// get args, env vars, read/write to stdin/stdout/stderr, get time, and
/// get random bytes.
///  
/// Intentionally excludes filesystem (except for stdin/stdout/stderr) and network
/// access for improved security and compatibility.
///
/// https://github.com/WebAssembly/WASI/blob/main/legacy/preview1/docs.md
pub struct WasiCtx {
    args: Vec<String>,
    env: Vec<String>,
    stdin_reader: Option<Box<dyn BufRead + Send + Sync>>,
    stdout_writer: Option<Box<dyn Write + Send + Sync>>,
    stderr_writer: Option<Box<dyn Write + Send + Sync>>,

    // Time when the host should resume the guest from an out-of-fuel yield.
    // If None, the guest should be started immediately. If Some, the guest
    // should be resumed after this time is reached.
    pub sleep_until: Option<Instant>,

    /// Whether the Wasi instance is current paused waiting for stdin. If true,
    /// the host may choose to wait for stdin to be ready before resuming execution.
    pub awaiting_stdin: bool,

    memory: Option<wasmer::Memory>,
}

#[derive(Debug, Error)]
pub enum WasiError {
    #[error("Instantiation Error: {0}")]
    InstantiationError(#[from] wasmer::InstantiationError),
    #[error("Export Error: {0}")]
    ExportError(#[from] wasmer::ExportError),
}

impl Default for WasiCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl WasiCtx {
    pub fn new() -> Self {
        Self {
            args: vec![],
            env: vec![],
            stdin_reader: None,
            stdout_writer: None,
            stderr_writer: None,
            sleep_until: None,
            awaiting_stdin: false,
            memory: None,
        }
    }

    #[allow(dead_code)]
    pub fn add_arg(mut self, arg: &str) -> Self {
        self.args.push(arg.to_string());
        self
    }

    #[allow(dead_code)]
    pub fn add_env(mut self, key: &str, value: &str) -> Self {
        self.env.push(format!("{}={}", key, value));
        self
    }

    pub fn set_stdin<R: Read + Send + Sync + 'static>(mut self, reader: R) -> Self {
        let reader = std::io::BufReader::new(reader);
        self.stdin_reader = Some(Box::new(reader));
        self
    }

    pub fn set_stdout<W: Write + Send + Sync + 'static>(mut self, writer: W) -> Self {
        self.stdout_writer = Some(Box::new(writer));
        self
    }

    pub fn set_stderr<W: Write + Send + Sync + 'static>(mut self, writer: W) -> Self {
        self.stderr_writer = Some(Box::new(writer));
        self
    }

    #[allow(clippy::result_large_err)]
    pub fn into_fn(
        self,
        mut store: &mut wasmer::Store,
        module: &wasmer::Module,
    ) -> Result<wasmer::Function, WasiError> {
        let ctx = wasmer::FunctionEnv::new(&mut store, self);
        let imports = wasmer::imports! {
            "wasi_snapshot_preview1" => {
                "args_get" => wasmer::Function::new_typed_with_env(&mut store, &ctx, args_get),
                "args_sizes_get" => wasmer::Function::new_typed_with_env(&mut store, &ctx, args_sizes_get),
                "environ_get" => wasmer::Function::new_typed_with_env(&mut store, &ctx, env_get),
                "environ_sizes_get" => wasmer::Function::new_typed_with_env(&mut store, &ctx, env_sizes_get),
                "fd_read" => wasmer::Function::new_typed_with_env(&mut store, &ctx, fd_read),
                "fd_write" => wasmer::Function::new_typed_with_env(&mut store, &ctx, fd_write),
                "fd_fdstat_get" => wasmer::Function::new_typed_with_env(&mut store, &ctx, fd_fdstat_get),
                "fd_close" => wasmer::Function::new_typed_with_env(&mut store, &ctx, fd_close),
                "clock_time_get" => wasmer::Function::new_typed_with_env(&mut store, &ctx, clock_time_get),
                "random_get" => wasmer::Function::new_typed_with_env(&mut store, &ctx, random_get),
                "poll_oneoff" => wasmer::Function::new_typed_with_env(&mut store, &ctx, poll_oneoff),
                "sched_yield" => wasmer::Function::new_typed_with_env(&mut store, &ctx, sched_yield),
                "proc_raise" => wasmer::Function::new_typed_with_env(&mut store, &ctx, proc_raise),
                "proc_exit" => wasmer::Function::new_typed_with_env(&mut store, &ctx, proc_exit),
            }
        };

        let instance = wasmer::Instance::new(&mut store, module, &imports)?;
        let memory = instance.exports.get_memory("memory")?;
        ctx.as_mut(&mut store).set_memory(memory.clone());

        let start_fn = instance.exports.get_function("_start")?;

        Ok(start_fn.clone())
    }

    fn set_memory(&mut self, memory: wasmer::Memory) {
        self.memory = Some(memory);
    }
}

#[repr(i32)]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
enum Errno {
    Success = 0,
    Access = 2,
    Again = 6,
    Badf = 8,
    Fault = 21,
    Inval = 28,
    Io = 29,
}

pub fn args_get(mut env: FunctionEnvMut<WasiCtx>, argv: i32, argv_buf: i32) -> i32 {
    trace!("wasi args_get({}, {})", argv, argv_buf);

    let (ctx, store) = env.data_and_store_mut();
    let memory = ctx.memory.as_ref().expect("Memory not initialized");
    let view = memory.view(&store);

    // Pointer in guest memory to an array of pointers that will be filled with the start of each argument string.
    let mut argv_ptr = argv as u64;
    // Pointer in guest memory to a buffer that will be filled with the `\0`-terminated argument strings.
    let mut buf_ptr = argv_buf as u64;

    for arg in &ctx.args {
        if write_pointer_and_string(&view, &mut argv_ptr, &mut buf_ptr, arg).is_err() {
            return Errno::Fault as i32;
        }
    }

    Errno::Success as i32
}

pub fn args_sizes_get(mut env: FunctionEnvMut<WasiCtx>, offset0: i32, offset1: i32) -> i32 {
    trace!("wasi args_sizes_get({}, {})", offset0, offset1);

    let (ctx, store) = env.data_and_store_mut();
    let memory = ctx.memory.as_ref().expect("Memory not initialized");
    let view = memory.view(&store);

    let argc = ctx.args.len() as u64;
    let argv_buf_len: u64 = ctx.args.iter().map(|s| s.len() as u64 + 1).sum(); // +1 for null terminator

    if view.write(offset0 as u64, &argc.to_le_bytes()).is_err() {
        return Errno::Fault as i32;
    }
    if view
        .write(offset1 as u64, &argv_buf_len.to_le_bytes())
        .is_err()
    {
        return Errno::Fault as i32;
    }

    Errno::Success as i32
}

/// Read environment variable data. The sizes of the buffers should match that
/// returned by environ_sizes_get. Key/value pairs are expected to be joined
/// with =s, and terminated with \0s.
pub fn env_get(mut env: FunctionEnvMut<WasiCtx>, environ: i32, environ_buf: i32) -> i32 {
    trace!("wasi environ_get({}, {})", environ, environ_buf);

    let (ctx, store) = env.data_and_store_mut();
    let memory = ctx.memory.as_ref().expect("Memory not initialized");
    let view = memory.view(&store);

    // Pointer in guest memory to an array of pointers that will be filled with the start of each environment string.
    let mut environ_ptr = environ as u64;
    // Pointer in guest memory to a buffer that will be filled with the `\0`-terminated environment strings.
    let mut buf_ptr = environ_buf as u64;

    for var in &ctx.env {
        if write_pointer_and_string(&view, &mut environ_ptr, &mut buf_ptr, var).is_err() {
            return Errno::Fault as i32;
        }
    }

    Errno::Success as i32
}

/// Returns the number of environment variable arguments and the size of the environment variable data.
pub fn env_sizes_get(mut env: FunctionEnvMut<WasiCtx>, offset0: i32, offset1: i32) -> i32 {
    trace!("wasi environ_sizes_get({}, {})", offset0, offset1);

    let (ctx, store) = env.data_and_store_mut();
    let memory = ctx.memory.as_ref().expect("Memory not initialized");
    let view = memory.view(&store);

    let envc = ctx.env.len() as u64;
    let env_buf_len: u64 = ctx.env.iter().map(|s| s.len() as u64 + 1).sum(); // +1 for null terminator

    if view.write(offset0 as u64, &envc.to_le_bytes()).is_err() {
        return Errno::Fault as i32;
    }
    if view
        .write(offset1 as u64, &env_buf_len.to_le_bytes())
        .is_err()
    {
        return Errno::Fault as i32;
    }

    Errno::Success as i32
}

pub fn fd_read(
    mut env: FunctionEnvMut<WasiCtx>,
    fd: u32,
    iov_ptr: i32,
    iov_len: i32,
    nread_ptr: i32,
) -> i32 {
    trace!(
        "wasi fd_read({}, {}, {}, {})",
        fd, iov_ptr, iov_len, nread_ptr
    );

    if fd != 0 {
        return Errno::Badf as i32;
    }

    let (ctx, store) = env.data_and_store_mut();
    let memory = ctx.memory.as_ref().expect("Memory not initialized");
    let view = memory.view(&store);

    let mut total_read = 0u64;

    for i in 0..iov_len {
        let base = (iov_ptr as u64) + (i as u64 * 8);
        let mut buf_bytes = [0u8; 4];

        view.read(base, &mut buf_bytes).unwrap();
        let buf_addr = u32::from_le_bytes(buf_bytes) as u64;

        view.read(base + 4, &mut buf_bytes).unwrap();
        let buf_len = u32::from_le_bytes(buf_bytes) as usize;

        // Allocate host buffer
        let mut host_buf = vec![0u8; buf_len];

        // Narrow scope: borrow reader only while calling `read`
        let n = {
            match ctx.stdin_reader.as_mut() {
                Some(r) => match r.read(&mut host_buf) {
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // trace!("fd_read: WouldBlock, yielding to host");
                        // ctx.awaiting_stdin = true;
                        // let _ = sched_yield(caller);
                        return Errno::Again as i32;
                    }
                    Err(_) => return Errno::Fault as i32,
                },
                None => return Errno::Badf as i32,
            }
        }; // <- borrow of ctx ends here

        if n == 0 {
            trace!("fd_read: EOF");
            break;
        }

        total_read += n as u64;
        view.write(buf_addr, &host_buf[..n]).unwrap();
        if n < buf_len {
            break; // short read
        }
    }

    view.write(nread_ptr as u64, &(total_read).to_le_bytes())
        .unwrap();

    trace!("fd_read: total_read={}", total_read);
    Errno::Success as i32
}

pub fn fd_write(
    mut env: FunctionEnvMut<WasiCtx>,
    fd: i32,
    ciov_ptr: i32,
    ciov_len: i32,
    nwrite_ptr: i32,
) -> i32 {
    trace!(
        "wasi fd_write({}, {}, {}, {})",
        fd, ciov_ptr, ciov_len, nwrite_ptr
    );

    let (ctx, store) = env.data_and_store_mut();
    let memory = ctx.memory.as_ref().expect("Memory not initialized");
    let view = memory.view(&store);

    let mut total_written = 0u32;

    let writer = match fd {
        1 => ctx.stdout_writer.as_mut(),
        2 => ctx.stderr_writer.as_mut(),
        _ => return Errno::Badf as i32,
    };

    let Some(writer) = writer else {
        return Errno::Badf as i32;
    };

    let mut scratch = [0u8; 8192];
    for i in 0..ciov_len {
        let base = (ciov_ptr as u64) + (i as u64 * 8);

        // Read the ciovec (ptr: u32, len: u32)
        let mut cio_buf = [0u8; 8];
        if view.read(base, &mut cio_buf).is_err() {
            return Errno::Fault as i32;
        }

        let buf_addr = u32::from_le_bytes(cio_buf[0..4].try_into().unwrap()) as u64;
        let buf_len = u32::from_le_bytes(cio_buf[4..8].try_into().unwrap()) as usize;

        if buf_len == 0 {
            continue;
        }

        // Process iovecs in chunks with the scratch buffer
        let mut written = 0;
        while written < buf_len {
            let to_copy = (buf_len - written).min(scratch.len());
            if view
                .read(buf_addr + written as u64, &mut scratch[..to_copy])
                .is_err()
            {
                return Errno::Fault as i32;
            }

            match writer.write(&scratch[..to_copy]) {
                Ok(0) => break, // Writer closed
                Ok(n) => {
                    written += n;
                    total_written += n as u32;
                    if n < to_copy {
                        break; // partial write, stop
                    }
                }
                Err(_) => return Errno::Io as i32,
            }
        }

        //? If we couldn't write the full iovec, stop processing further iovecs
        if written < buf_len {
            break;
        }
    }

    // Write total_written into nwrite_ptr
    if view
        .write(nwrite_ptr as u64, &total_written.to_le_bytes())
        .is_err()
    {
        return Errno::Fault as i32;
    }

    Errno::Success as i32
}

pub fn fd_fdstat_get(mut env: FunctionEnvMut<WasiCtx>, fd: i32, buf_ptr: i32) -> i32 {
    trace!("wasi fd_fdstat_get({}, {})", fd, buf_ptr);

    let (ctx, store) = env.data_and_store_mut();
    let memory = ctx.memory.as_ref().expect("Memory not initialized");
    let view = memory.view(&store);

    // Constants from wasi_snapshot_preview1
    const FILETYPE_CHARACTER_DEVICE: u8 = 2;

    // fdflags (none set)
    let fs_flags: u16 = 0;

    // rights (for simplicity, we just allow read or write)
    const RIGHTS_FD_READ: u64 = 1 << 1;
    const RIGHTS_FD_WRITE: u64 = 1 << 6;

    let (filetype, rights_base) = match fd {
        0 => (FILETYPE_CHARACTER_DEVICE, RIGHTS_FD_READ),
        1 | 2 => (FILETYPE_CHARACTER_DEVICE, RIGHTS_FD_WRITE),
        _ => return Errno::Badf as i32,
    };

    let rights_inheriting: u64 = 0;

    let mut buf = [0u8; 24];
    buf[0] = filetype;
    buf[1..3].copy_from_slice(&fs_flags.to_le_bytes());
    buf[8..16].copy_from_slice(&rights_base.to_le_bytes());
    buf[16..24].copy_from_slice(&rights_inheriting.to_le_bytes());

    if view.write(buf_ptr as u64, &buf).is_err() {
        return Errno::Fault as i32;
    }

    Errno::Success as i32
}

/// Close a file descriptor. For now we only support closing stdin, stdout, stderr.
pub fn fd_close(mut env: FunctionEnvMut<WasiCtx>, fd: i32) -> i32 {
    trace!("wasi fd_close({})", fd);

    let ctx = env.data_mut();

    match fd {
        0 => ctx.stdin_reader = None,
        1 => ctx.stdout_writer = None,
        2 => ctx.stderr_writer = None,
        _ => return Errno::Badf as i32,
    }

    Errno::Success as i32
}

pub fn clock_time_get(
    mut env: FunctionEnvMut<WasiCtx>,
    clock_id: i32,
    _precision: i64,
    result_ptr: i32,
) -> i32 {
    trace!("wasi clock_time_get({}, _, {})", clock_id, result_ptr);

    let (ctx, store) = env.data_and_store_mut();
    let memory = ctx.memory.as_ref().expect("Memory not initialized");
    let view = memory.view(&store);

    let now = match clock_id {
        // Realtime: nanoseconds since UNIX epoch
        0 | 1 => {
            match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                Ok(dur) => dur.as_nanos() as u64,
                Err(_) => return Errno::Inval as i32, // time before epoch shouldn't happen
            }
        }
        _ => {
            warn!("unsupported clock_id {}", clock_id);
            return Errno::Inval as i32; // unsupported clock
        }
    };

    if view.write(result_ptr as u64, &now.to_le_bytes()).is_err() {
        return Errno::Fault as i32;
    }
    Errno::Success as i32
}

pub fn random_get(mut env: FunctionEnvMut<WasiCtx>, buf_ptr: i32, buf_len: i32) -> i32 {
    trace!("wasi random_get({}, {})", buf_ptr, buf_len);

    let (ctx, store) = env.data_and_store_mut();
    let memory = ctx.memory.as_ref().expect("Memory not initialized");
    let view = memory.view(&store);

    let mut buf = vec![0u8; buf_len as usize];
    if let Err(e) = getrandom::fill(&mut buf) {
        eprintln!("random_get failed: {:?}", e);
        return Errno::Io as i32;
    }

    if view.write(buf_ptr as u64, &buf).is_err() {
        return Errno::Fault as i32;
    }

    Errno::Success as i32
}

const CLOCK_EVENT_TYPE: u8 = 0;

pub fn poll_oneoff(
    mut env: FunctionEnvMut<WasiCtx>,
    subs_ptr: i32,
    events_ptr: i32,
    nsubscriptions: i32,
    result_ptr: i32,
) -> i32 {
    trace!(
        "wasi poll_oneoff({}, {}, {}, {})",
        subs_ptr, events_ptr, nsubscriptions, result_ptr
    );

    if nsubscriptions == 0 {
        error!("poll_oneoff called with zero subscriptions");
        return Errno::Inval as i32;
    }

    let (ctx, store) = env.data_and_store_mut();
    let memory = ctx.memory.as_ref().expect("Memory not initialized");
    let view = memory.view(&store);

    let mut clock_subscriptions = Vec::new();
    for i in 0..nsubscriptions {
        // Each subscription is 48 bytes
        let sub_offset = (subs_ptr as usize) + (i as usize * 48);

        // Read the subscription struct
        // subscription layout (48 bytes total):
        // - userdata: u64 at offset 0
        // - type: u8 at offset 8 (eventtype: 0=clock, 1=fd_read, 2=fd_write)
        // - padding: 7 bytes
        // - union data: 32 bytes at offset 16
        //
        // For clock subscription (subscription_clock at offset 16):
        // - id: u32 (clockid) at offset 16
        // - padding: 4 bytes
        // - timeout: u64 (timestamp) at offset 24
        // - precision: u64 (timestamp) at offset 32
        // - flags: u16 (subclockflags) at offset 40

        let mut sub_bytes = [0u8; 48];
        view.read(sub_offset as u64, &mut sub_bytes).unwrap();

        let userdata = u64::from_le_bytes(sub_bytes[0..8].try_into().unwrap());
        let sub_type = sub_bytes[8];

        if sub_type != CLOCK_EVENT_TYPE {
            warn!(
                "poll_oneoff: only clock subscriptions supported, got type {}",
                sub_type
            );
            continue;
        }

        let clock_id = u32::from_le_bytes(sub_bytes[16..20].try_into().unwrap());
        let timeout_ns = u64::from_le_bytes(sub_bytes[24..32].try_into().unwrap());
        let precision_ns = u64::from_le_bytes(sub_bytes[32..40].try_into().unwrap());
        let flags = u16::from_le_bytes(sub_bytes[40..42].try_into().unwrap());

        if clock_id > 1 {
            // Only realtime (0) and monotonic (1)
            warn!("poll_oneoff: unsupported clock_id {}", clock_id);
            continue;
        }

        let is_absolute = (flags & 1) != 0;
        trace!(
            "poll_oneoff: clock_id={}, timeout={}, precision={}, flags={:#x}, is_absolute={}",
            clock_id, timeout_ns, precision_ns, flags, is_absolute
        );

        clock_subscriptions.push((userdata, clock_id, timeout_ns, is_absolute));
    }

    info!(
        "poll_oneoff: {} clock subscriptions",
        clock_subscriptions.len()
    );

    if clock_subscriptions.is_empty() {
        warn!("poll_oneoff: no valid clock subscriptions found");
        return Errno::Inval as i32;
    }

    // Convert all relative timeouts to absolute deadlines
    let now_ns = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(dur) => dur.as_nanos() as u64,
        Err(_) => return Errno::Inval as i32, // time before epoch shouldn't happen
    };

    clock_subscriptions.iter_mut().for_each(|sub| {
        if !sub.3 {
            sub.2 = now_ns.saturating_add(sub.2);
        }
    });

    // Find the earliest deadline
    let (earliest_userdata, _earliest_clock_id, earliest_deadline_ns, _) =
        clock_subscriptions.iter().min_by_key(|sub| sub.2).unwrap();

    // Just blocking sleep until the earliest deadline is hit.
    if earliest_deadline_ns > &now_ns {
        let wait_duration = Duration::from_nanos(earliest_deadline_ns - now_ns);

        blocking_sleep(wait_duration);
    }

    // Write the event for the earliest deadline
    // size: 32
    // - userdata: at offset 0
    // - error: at offset 8
    // - type: at offset 10
    // - fd_readwrite: at offset 16 (empty for clock events)
    let mut event_buf = [0u8; 32];
    event_buf[0..8].copy_from_slice(&earliest_userdata.to_le_bytes());
    event_buf[8..10].copy_from_slice(&(Errno::Success as u16).to_le_bytes());
    event_buf[10] = CLOCK_EVENT_TYPE;
    // fd_readwrite is empty for clock events

    info!(
        "Writing event: userdata={}, error={}, type={}",
        earliest_userdata,
        Errno::Success as u16,
        CLOCK_EVENT_TYPE
    );
    info!("Event buffer: {:?}", &event_buf[..16]);

    let event_offset = events_ptr as usize;
    view.write(event_offset as u64, &event_buf).unwrap();

    // Write number of events (1) into result_ptr
    view.write(result_ptr as u64, &1u32.to_le_bytes()).unwrap();

    Errno::Success as i32
}

pub fn sched_yield(mut _env: FunctionEnvMut<WasiCtx>) -> i32 {
    trace!("wasi sched_yield()");
    Errno::Success as i32
}

pub fn proc_raise(_: FunctionEnvMut<WasiCtx>, sig: i32) -> Result<(), RuntimeError> {
    info!("wasi proc_raise({})", sig);

    // https://github.com/WebAssembly/WASI/blob/main/legacy/preview1/docs.md#signal
    match sig {
        0 | 13 | 16 | 17 | 22 | 27 => {
            // Ignored signals
            Ok(())
        }
        _ => {
            // Termination signals
            Err(RuntimeError::new(format!("exit: {}", sig + 128)))
        }
    }
}

pub fn proc_exit(_: FunctionEnvMut<WasiCtx>, status: i32) -> Result<(), RuntimeError> {
    info!("wasi proc_exit({})", status);
    Err(RuntimeError::new(format!("exit: {}", status)))
}

fn write_pointer_and_string(
    view: &wasmer::MemoryView,
    table_ptr: &mut u64,
    buf_ptr: &mut u64,
    s: &str,
) -> Result<(), Errno> {
    // 1. Write pointer into table
    let ptr_le = (*buf_ptr as u32).to_le_bytes();
    view.write(*table_ptr, &ptr_le).map_err(|_| Errno::Fault)?;
    *table_ptr += 4;

    // 2. Write string+null into buffer
    let mut tmp = Vec::with_capacity(s.len() + 1);
    tmp.extend_from_slice(s.as_bytes());
    tmp.push(0); // null terminator
    view.write(*buf_ptr, &tmp).map_err(|_| Errno::Fault)?;
    *buf_ptr += tmp.len() as u64;

    Ok(())
}
