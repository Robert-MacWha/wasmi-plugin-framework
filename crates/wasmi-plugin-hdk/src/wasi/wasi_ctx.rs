use std::io::{BufReader, Read, Write};
use thiserror::Error;
use tracing::{error, info, trace, warn};
use wasmer::{FunctionEnvMut, RuntimeError};
use web_time::{Duration, Instant, SystemTime};

use crate::time::blocking_sleep;

pub trait WasiReader: Read + Send + Sync {
    /// Blocks the current thread until the reader is ready to read, or the timeout is reached.
    fn wait_ready(&self, timeout: Option<Duration>);

    /// Returns true if the reader is ready to read without blocking.
    fn is_ready(&self) -> bool;
}

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
    stdin_reader: Option<Box<BufReader<dyn WasiReader>>>,
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

    pub fn set_stdin<R: WasiReader + Send + Sync + 'static>(mut self, reader: R) -> Self {
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
        let (ctx, imports) = self.get_imports(store);

        let instance = wasmer::Instance::new(&mut store, module, &imports)?;
        let memory = instance.exports.get_memory("memory")?;
        ctx.as_mut(&mut store).set_memory(memory.clone());

        let start_fn = instance.exports.get_function("_start")?;

        Ok(start_fn.clone())
    }

    pub fn get_imports(
        self,
        mut store: &mut wasmer::Store,
    ) -> (wasmer::FunctionEnv<WasiCtx>, wasmer::Imports) {
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

        (ctx, imports)
    }

    pub fn set_memory(&mut self, memory: wasmer::Memory) {
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

            // Print the data to be copied as a string
            info!(
                "fd_write: writing {} bytes to fd {}: {:?}",
                to_copy,
                fd,
                String::from_utf8_lossy(&scratch[..to_copy])
            );
            match writer.write(&scratch[..to_copy]) {
                Ok(0) => {
                    info!("fd_write: writer returned 0 bytes written, stopping");
                    break;
                }
                Ok(n) => {
                    info!("fd_write: wrote {} bytes to fd {}", n, fd);
                    written += n;
                    total_written += n as u32;
                    if n < to_copy {
                        break; // partial write, stop
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    info!("fd_write: writer would block");
                    // Spin until writable
                    continue;
                }
                Err(_) => {
                    info!("fd_write: writer error");
                    return Errno::Io as i32;
                }
            }
        }

        //? If we couldn't write the full iovec, stop processing further iovecs
        if written < buf_len {
            info!("fd_write: short write, stopping further iovecs");
            break;
        }
    }

    info!("fd_write: total_written={}", total_written);
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
const FD_READ_EVENT_TYPE: u8 = 1;
#[allow(dead_code)]
const FD_WRITE_EVENT_TYPE: u8 = 2;

#[repr(C, packed)]
#[derive(Copy, Clone)]
struct WasiSubscription {
    userdata: u64,
    type_: u8,
    _padding: [u8; 7],
    union: WasiSubscriptionUnion,
}

#[repr(C, packed)]
#[derive(Copy, Clone)]
union WasiSubscriptionUnion {
    clock: WasiSubscriptionClock,
    fd_readwrite: WasiSubscriptionFdReadWrite,
}

#[repr(C, packed)]
#[derive(Copy, Clone)]
struct WasiSubscriptionClock {
    id: u32,
    _padding: [u8; 4],
    timeout: u64,
    precision: u64,
    flags: u16,
}

#[repr(C, packed)]
#[derive(Copy, Clone)]
struct WasiSubscriptionFdReadWrite {
    fd: u32,
}

#[repr(C, packed)]
struct WasiEvent {
    userdata: u64,
    error: u16,
    type_: u8,
    _padding: [u8; 5],
    fd_readwrite: WasiEventFdReadWrite,
}

#[repr(C, packed)]
struct WasiEventFdReadWrite {
    nbytes: u64,
    flags: u16,
    _padding: [u8; 6],
}

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

    //? Currently only supports a single clock / stdin subscription, and doesn't
    //? support stdout. Takes the earliest clock and the last stdin subscription.

    // TODO: Add stdout support (via similar wait_ready mechanism as stdin)
    // TODO: Support multiple file descriptors concurrently. Perhaps change
    // the wait_* mechanism to async and select across multiple futures?

    let mut stdin_sub: Option<WasiSubscriptionFdReadWrite> = None;
    let mut stdin_userdata: u64 = 0;

    // Time at which earliest clock sub will resolve
    let mut earliest_clock_sub_time: Option<u64> = None;
    let mut earliest_clock_sub: Option<WasiSubscriptionClock> = None;
    let mut clock_userdata: u64 = 0;
    for i in 0..nsubscriptions {
        // Each subscription is 48 bytes
        let sub_offset = (subs_ptr as usize) + (i as usize * 48);
        let mut sub_bytes = [0u8; 48];
        view.read(sub_offset as u64, &mut sub_bytes).unwrap();

        // Cast bytes to struct
        let sub = unsafe { &*(sub_bytes.as_ptr() as *const WasiSubscription) };

        match sub.type_ {
            CLOCK_EVENT_TYPE => {
                let clock = unsafe { sub.union.clock };
                let is_absolute = (clock.flags & 1) != 0;

                //? Convert to absolute time if required
                let timeout_ns = if is_absolute {
                    clock.timeout
                } else {
                    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                        Ok(dur) => dur.as_nanos() as u64 + clock.timeout,
                        Err(_) => return Errno::Inval as i32, // time before epoch shouldn't happen
                    }
                };

                //? Keep the earliest clock subscription
                if earliest_clock_sub_time.is_none()
                    || timeout_ns < earliest_clock_sub_time.unwrap()
                {
                    earliest_clock_sub_time = Some(timeout_ns);
                    earliest_clock_sub = Some(clock);
                    clock_userdata = sub.userdata;
                }
            }
            FD_READ_EVENT_TYPE => {
                let fd_read = unsafe { sub.union.fd_readwrite };

                // Only support stdin (fd 0) for now
                if fd_read.fd != 0 {
                    warn!("poll_oneoff: only fd_read on stdin (fd 0) is supported");
                    continue;
                }
                stdin_sub = Some(fd_read);
                stdin_userdata = sub.userdata;
            }
            _ => {
                warn!("poll_oneoff: unsupported subscription type {}", sub.type_);
                continue;
            }
        }
    }

    //? If no subscriptions were found, return error
    if stdin_sub.is_none() && earliest_clock_sub.is_none() {
        warn!("poll_oneoff: no valid subscriptions found");
        return Errno::Inval as i32;
    }

    let clock_duration = earliest_clock_sub_time.map(|deadline| {
        let now_ns = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        if deadline > now_ns {
            Duration::from_nanos(deadline - now_ns)
        } else {
            Duration::from_nanos(0)
        }
    });

    //? If there's a stdin subscription, wait on that with the clock as timeout. Otherwise just wait on the clock.
    if stdin_sub.is_some()
        && let Some(reader) = &ctx.stdin_reader
    {
        trace!("poll_oneoff: waiting for stdin or clock timeout...");
        let reader = reader.get_ref();
        reader.wait_ready(clock_duration);
    } else if let Some(duration) = clock_duration {
        trace!("poll_oneoff: waiting for clock timeout...");
        blocking_sleep(duration);
    }

    let mut events_written = 0;

    // 1. Check if Stdin is ready (Data available or Pipe Closed)
    if let Some(_sub) = stdin_sub
        && let Some(reader) = &ctx.stdin_reader
        && reader.get_ref().is_ready()
    {
        write_event(
            &view,
            events_ptr,
            events_written,
            stdin_userdata,
            FD_READ_EVENT_TYPE,
        );
        events_written += 1;
    }

    // 2. Check if Clock has expired
    if let Some(deadline_ns) = earliest_clock_sub_time {
        let now_ns = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        if now_ns >= deadline_ns {
            write_event(
                &view,
                events_ptr,
                events_written,
                clock_userdata,
                CLOCK_EVENT_TYPE,
            );
            events_written += 1;
        }
    }

    // Write number of events triggered to result_ptr
    view.write(result_ptr as u64, &events_written.to_le_bytes())
        .unwrap();

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

fn write_event(view: &wasmer::MemoryView, events_ptr: i32, index: u32, userdata: u64, type_: u8) {
    let event = WasiEvent {
        userdata,
        error: 0, // Errno::Success
        type_,
        _padding: [0; 5],
        fd_readwrite: WasiEventFdReadWrite {
            nbytes: 0,
            flags: 0,
            _padding: [0; 6],
        },
    };

    let offset = (events_ptr as u64) + (index as u64 * 32);
    let bytes: [u8; 32] = unsafe { std::mem::transmute(event) };
    view.write(offset, &bytes).unwrap();

    trace!(
        "poll_oneoff: Wrote event type {} for userdata {}",
        type_, userdata
    );
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
