//! A watchdog for the main loop (a modern addition, for bug reports). The main thread marks
//! when it is inside a window-event handler ([`enter`], dropped on the way out) and passes
//! named checkpoints ([`checkpoint`]) and the frame number on the way through a frame. A
//! thread of its own looks twice a second: when the main thread has been inside a handler
//! without passing a checkpoint for [`STALL`], the game is not responding, and the frame, the
//! last checkpoint, the game's state line ([`detail`]) and, on 64-bit Windows, the main
//! thread's native stack go to `cataclysm-crash.log` before anyone has to end the process.
//! A second report follows if it is still stuck later on, and a note if it comes back.
//!
//! Waiting for the next event between frames is not a stall (the loop is outside every
//! handler then), so a minimised window or a modal system loop does not report.

use std::sync::Mutex;
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// How long the main thread may go without a checkpoint inside a handler.
pub const STALL: Duration = Duration::from_secs(5);

/// When the reports of one stall are written (from its start).
const REPORT_AT: [Duration; 2] = [STALL, Duration::from_secs(20)];

struct State {
    /// The watched thread; calls from any other thread (tests in parallel) are ignored.
    watched: Option<ThreadId>,
    checkpoint: &'static str,
    /// What the outermost handler is.
    handler: &'static str,
    detail: String,
    frame: u64,
    depth: u32,
    /// The last progress while inside a handler.
    since: Option<Instant>,
    /// Reports written for the stall going on now.
    reports: usize,
}

static STATE: Mutex<State> = Mutex::new(State {
    watched: None,
    checkpoint: "start",
    handler: "",
    detail: String::new(),
    frame: 0,
    depth: 0,
    since: None,
    reports: 0,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The state, when the calling thread is the watched one.
fn watched_state() -> Option<std::sync::MutexGuard<'static, State>> {
    let s = state();
    (s.watched == Some(std::thread::current().id())).then_some(s)
}

/// Starts watching the calling thread; `on_stall` gets each report (the crash log in the
/// game, stderr and an abort in the hang tests).
pub fn install(on_stall: fn(&str)) {
    {
        let mut s = state();
        s.watched = Some(std::thread::current().id());
        s.depth = 0;
        s.since = None;
        s.reports = 0;
    }
    #[cfg(all(windows, target_arch = "x86_64"))]
    native::remember_main_thread();
    let spawned = std::thread::Builder::new()
        .name("watchdog".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(500));
            if let Some(report) = look() {
                on_stall(&report);
            }
        });
    if let Err(e) = spawned {
        eprintln!("cataclysm: no main-loop watchdog: {e}");
    }
}

/// The watchdog's look: a report when one is due.
fn look() -> Option<String> {
    let (stuck, frame, checkpoint, handler, detail) = {
        let mut s = state();
        let stuck = s.since.filter(|_| s.depth > 0)?.elapsed();
        if REPORT_AT.get(s.reports).is_none_or(|&at| stuck < at) {
            return None;
        }
        s.reports += 1;
        (stuck, s.frame, s.checkpoint, s.handler, s.detail.clone())
    };
    // The lock is free again before the main thread is stopped to read its stack.
    #[cfg(all(windows, target_arch = "x86_64"))]
    let stack = native::main_thread_stack();
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    let stack = String::from("  (no native stack on this platform)\n");
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Some(format!(
        "cataclysm {} (unix time {since}): the main loop has not responded for {:.1} s\n\
         frame {frame}, in {handler}, last checkpoint '{checkpoint}'\n\
         game: {detail}\n\nmain thread stack:\n{stack}",
        env!("CARGO_PKG_VERSION"),
        stuck.as_secs_f32(),
    ))
}

/// The main thread is inside a handler until the guard drops.
#[must_use]
pub struct Busy(());

impl Drop for Busy {
    fn drop(&mut self) {
        let Some(mut s) = watched_state() else {
            return;
        };
        s.depth = s.depth.saturating_sub(1);
        if s.depth == 0 {
            if s.reports > 0 {
                let stuck = s.since.map_or(0.0, |t| t.elapsed().as_secs_f32());
                let checkpoint = s.checkpoint;
                drop(s);
                crate::crash::append(&format!(
                    "cataclysm: the main loop responds again ({stuck:.1} s after checkpoint '{checkpoint}')\n"
                ));
                let Some(mut s) = watched_state() else {
                    return;
                };
                s.reports = 0;
                s.since = None;
                return;
            }
            s.since = None;
        }
    }
}

/// The main thread enters a handler (`what` names it).
pub fn enter(what: &'static str) -> Busy {
    if let Some(mut s) = watched_state() {
        if s.depth == 0 {
            s.handler = what;
            s.since = Some(Instant::now());
            s.reports = 0;
        }
        s.depth += 1;
        s.checkpoint = what;
    }
    Busy(())
}

/// The main thread got to `at`.
pub fn checkpoint(at: &'static str) {
    if let Some(mut s) = watched_state() {
        s.checkpoint = at;
        if s.depth > 0 && s.reports == 0 {
            s.since = Some(Instant::now());
        }
    }
}

/// The frame being made.
pub fn frame(n: u64) {
    if let Some(mut s) = watched_state() {
        s.frame = n;
    }
}

/// The game's state in a line (what is up: an NIS, the sensors manager, the bars, ...).
pub fn detail(line: impl FnOnce() -> String) {
    if let Some(mut s) = watched_state() {
        s.detail = line();
    }
}

/// The report for the game: into the crash log.
pub fn to_crash_log(report: &str) {
    crate::crash::append(report);
}

/// The main thread's stack on 64-bit Windows: the thread is suspended just long enough to
/// walk its frames with the system's unwind tables (nothing is allocated meanwhile: it may hold
/// the heap's lock), then the addresses are named with dbghelp when it has the symbols.
#[cfg(all(windows, target_arch = "x86_64"))]
mod native {
    use std::ffi::c_void;
    use std::fmt::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const THREAD_ACCESS: u32 = 0x0002 | 0x0008 | 0x0040; // suspend/resume, get context, query
    const CONTEXT_FULL: u32 = 0x0010_000B;
    const CONTEXT_FLAGS_AT: usize = 0x30;
    const RSP_AT: usize = 0x98;
    const RIP_AT: usize = 0xF8;
    const MAX_FRAMES: usize = 64;

    /// The x64 `CONTEXT` record, as bytes at its alignment.
    #[repr(C, align(16))]
    struct Context([u8; 0x4D0]);

    impl Context {
        fn get(&self, at: usize) -> u64 {
            u64::from_ne_bytes(self.0[at..at + 8].try_into().unwrap_or_default())
        }
        fn set(&mut self, at: usize, v: u64) {
            self.0[at..at + 8].copy_from_slice(&v.to_ne_bytes());
        }
    }

    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn GetCurrentProcess() -> *mut c_void;
        fn OpenThread(access: u32, inherit: i32, id: u32) -> *mut c_void;
        fn SuspendThread(thread: *mut c_void) -> u32;
        fn ResumeThread(thread: *mut c_void) -> u32;
        fn GetThreadContext(thread: *mut c_void, context: *mut Context) -> i32;
        fn RtlLookupFunctionEntry(pc: u64, base: *mut u64, history: *mut c_void) -> *mut c_void;
        fn RtlVirtualUnwind(
            kind: u32,
            base: u64,
            pc: u64,
            entry: *mut c_void,
            context: *mut Context,
            handler_data: *mut *mut c_void,
            frame: *mut u64,
            pointers: *mut c_void,
        ) -> *mut c_void;
        fn GetModuleHandleExW(flags: u32, address: *const c_void, module: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }

    static MAIN: AtomicUsize = AtomicUsize::new(0);

    pub fn remember_main_thread() {
        // SAFETY: opens a handle to the calling thread; it is kept for the program's life.
        let h = unsafe { OpenThread(THREAD_ACCESS, 0, GetCurrentThreadId()) };
        MAIN.store(h as usize, Ordering::SeqCst);
    }

    /// The return addresses of the main thread, innermost first.
    fn walk() -> Result<([u64; MAX_FRAMES], usize), &'static str> {
        let thread = MAIN.load(Ordering::SeqCst) as *mut c_void;
        if thread.is_null() {
            return Err("no handle to the main thread");
        }
        let mut pcs = [0u64; MAX_FRAMES];
        let mut n = 0;
        let mut ctx = Context([0; 0x4D0]);
        ctx.0[CONTEXT_FLAGS_AT..CONTEXT_FLAGS_AT + 4].copy_from_slice(&CONTEXT_FULL.to_ne_bytes());
        // SAFETY: the handle has the suspend and context rights. While the thread is stopped
        // only the system's unwinder runs here, on that thread's own (valid) stack, and nothing
        // allocates.
        unsafe {
            if SuspendThread(thread) == u32::MAX {
                return Err("SuspendThread failed");
            }
            if GetThreadContext(thread, &mut ctx) != 0 {
                while n < MAX_FRAMES {
                    let pc = ctx.get(RIP_AT);
                    if pc == 0 {
                        break;
                    }
                    pcs[n] = pc;
                    n += 1;
                    let mut base = 0u64;
                    let entry = RtlLookupFunctionEntry(pc, &mut base, std::ptr::null_mut());
                    if entry.is_null() {
                        // A leaf: the return address is on top of the stack.
                        let sp = ctx.get(RSP_AT);
                        if sp == 0 || !sp.is_multiple_of(8) {
                            break;
                        }
                        ctx.set(RIP_AT, *(sp as *const u64));
                        ctx.set(RSP_AT, sp + 8);
                    } else {
                        let mut data = std::ptr::null_mut();
                        let mut frame = 0u64;
                        RtlVirtualUnwind(
                            0,
                            base,
                            pc,
                            entry,
                            &mut ctx,
                            &mut data,
                            &mut frame,
                            std::ptr::null_mut(),
                        );
                    }
                }
            }
            ResumeThread(thread);
        }
        Ok((pcs, n))
    }

    fn module_of(address: u64) -> Option<(String, u64)> {
        let mut module = std::ptr::null_mut();
        // SAFETY: asks for the module holding `address` without touching its reference count.
        let found = unsafe { GetModuleHandleExW(0x4 | 0x2, address as *const c_void, &mut module) };
        if found == 0 {
            return None;
        }
        let mut name = [0u16; 512];
        // SAFETY: `name` has room for the length passed.
        let n = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) };
        let path = String::from_utf16_lossy(&name[..(n as usize).min(name.len())]);
        let file = path.rsplit('\\').next().unwrap_or(&path).to_string();
        Some((file, address - module as u64))
    }

    type SymInitialize = unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32;
    type SymSetOptions = unsafe extern "system" fn(u32) -> u32;
    type SymFromAddr = unsafe extern "system" fn(*mut c_void, u64, *mut u64, *mut u8) -> i32;
    type SymGetLine = unsafe extern "system" fn(*mut c_void, u64, *mut u32, *mut Line) -> i32;

    /// `IMAGEHLP_LINE64`.
    #[repr(C)]
    struct Line {
        size: u32,
        key: *mut c_void,
        line: u32,
        file: *const u8,
        address: u64,
    }

    struct Symbols {
        process: *mut c_void,
        from_addr: SymFromAddr,
        line: Option<SymGetLine>,
    }

    fn symbols() -> Option<Symbols> {
        let name: Vec<u16> = "dbghelp.dll\0".encode_utf16().collect();
        // SAFETY: loads the system's dbghelp and looks up its documented entry points, cast
        // to their documented signatures.
        unsafe {
            let lib = LoadLibraryW(name.as_ptr());
            if lib.is_null() {
                return None;
            }
            let get = |n: &[u8]| GetProcAddress(lib, n.as_ptr());
            let (init, opts, from) = (
                get(b"SymInitializeW\0"),
                get(b"SymSetOptions\0"),
                get(b"SymFromAddr\0"),
            );
            if init.is_null() || opts.is_null() || from.is_null() {
                return None;
            }
            let line = get(b"SymGetLineFromAddr64\0");
            let process = GetCurrentProcess();
            let opts: SymSetOptions = std::mem::transmute(opts);
            opts(0x2 | 0x4 | 0x10); // undecorated names, deferred loads, lines
            let init: SymInitialize = std::mem::transmute(init);
            // The symbols next to the executable. It may already be initialised (the standard
            // library's backtraces): fine.
            let dir: Vec<u16> = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned()))
                .unwrap_or_default()
                .encode_utf16()
                .chain([0])
                .collect();
            init(process, dir.as_ptr(), 1);
            Some(Symbols {
                process,
                from_addr: std::mem::transmute::<*mut c_void, SymFromAddr>(from),
                line: (!line.is_null())
                    .then(|| std::mem::transmute::<*mut c_void, SymGetLine>(line)),
            })
        }
    }

    fn name_of(s: &Symbols, pc: u64) -> Option<String> {
        // `SYMBOL_INFO` (88 bytes) with room for the name after it.
        const MAX_NAME: u32 = 512;
        let mut buf = [0u64; (88 + MAX_NAME as usize) / 8 + 1];
        let p = buf.as_mut_ptr() as *mut u8;
        let mut displacement = 0u64;
        // SAFETY: the buffer is a zeroed `SYMBOL_INFO` with its size and the name's room set.
        let name = unsafe {
            *(p as *mut u32) = 88;
            *(p.add(80) as *mut u32) = MAX_NAME;
            if (s.from_addr)(s.process, pc, &mut displacement, p) == 0 {
                return None;
            }
            let len = (*(p.add(76) as *const u32)).min(MAX_NAME) as usize;
            String::from_utf8_lossy(std::slice::from_raw_parts(p.add(84), len)).into_owned()
        };
        let mut out = format!("{name}+0x{displacement:x}");
        if let Some(get_line) = s.line {
            let mut line = Line {
                size: std::mem::size_of::<Line>() as u32,
                key: std::ptr::null_mut(),
                line: 0,
                file: std::ptr::null(),
                address: 0,
            };
            let mut d = 0u32;
            // SAFETY: `line` is an `IMAGEHLP_LINE64` with its size set; the file name it
            // returns is a C string owned by dbghelp.
            unsafe {
                if get_line(s.process, pc, &mut d, &mut line) != 0 && !line.file.is_null() {
                    let file = std::ffi::CStr::from_ptr(line.file.cast()).to_string_lossy();
                    let _ = write!(out, " ({file}:{})", line.line);
                }
            }
        }
        Some(out)
    }

    pub fn main_thread_stack() -> String {
        let (pcs, n) = match walk() {
            Ok(w) => w,
            Err(e) => return format!("  ({e})\n"),
        };
        let syms = symbols();
        let mut out = String::new();
        for (i, &pc) in pcs[..n].iter().enumerate() {
            // A return address points after its call: named from the call itself.
            let at = if i == 0 { pc } else { pc - 1 };
            let module =
                module_of(pc).map_or_else(|| "?".to_string(), |(m, off)| format!("{m}+0x{off:x}"));
            let name = syms
                .as_ref()
                .and_then(|s| name_of(s, at))
                .unwrap_or_default();
            let _ = writeln!(out, "  {i:2}: 0x{pc:016x} {module} {name}");
        }
        if n == 0 {
            out.push_str("  (no frames)\n");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stall inside a handler is reported, with the checkpoint and (on Windows x64) the
    /// stuck thread's own stack; leaving the handler ends it.
    #[test]
    fn a_stuck_handler_is_reported_with_its_stack() {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel::<String>();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            {
                let mut s = state();
                s.watched = Some(std::thread::current().id());
                s.depth = 0;
                s.reports = 0;
            }
            #[cfg(all(windows, target_arch = "x86_64"))]
            native::remember_main_thread();
            let _busy = enter("test handler");
            frame(42);
            detail(|| "stuck on purpose".into());
            checkpoint("waiting for the test");
            // Pretend the stall started long ago.
            state().since = Some(Instant::now() - STALL - Duration::from_millis(10));
            tx.send(String::new()).unwrap();
            go_rx.recv().unwrap();
        });
        rx.recv().unwrap();
        let report = look().expect("a report for the stall");
        // Not watched any more: the way out writes no note to the crash log.
        state().watched = None;
        go_tx.send(()).unwrap();
        worker.join().unwrap();
        eprintln!("{report}");
        assert!(report.contains("frame 42"), "{report}");
        assert!(report.contains("'waiting for the test'"), "{report}");
        assert!(report.contains("stuck on purpose"), "{report}");
        #[cfg(all(windows, target_arch = "x86_64"))]
        assert!(report.contains(" 1: 0x"), "no stack: {report}");
        // Gone: nothing more to report.
        state().depth = 0;
        assert!(look().is_none());
    }
}
