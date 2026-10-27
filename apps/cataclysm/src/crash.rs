//! Crash reports: a panic hook that writes the message, where it happened and a backtrace to
//! `cataclysm-crash.log` (next to the executable, or in the user's config directory when that
//! is not writable) and to stderr. A panic on the main thread then ends the game at once with a
//! failure code, so it can never unwind into a foreign frame (a window or driver callback),
//! where it would abort the process with nothing to show for it. A panic on another thread
//! (the audio device's) is logged and left to that thread's own handler.
//!
//! On Windows the native failures are reported too: a C or C++ `abort()` (from a driver, the
//! C runtime's `std::terminate`, ...) through a `SIGABRT` handler before the runtime ends the
//! process, and an exception nobody handles (an access violation, an illegal instruction) through
//! the unhandled-exception filter, each with the exception code, the module it happened in and
//! a native backtrace.

use std::backtrace::Backtrace;
use std::io::Write;
use std::panic::PanicHookInfo;
use std::path::PathBuf;

/// The report's file name.
pub const LOG_NAME: &str = "cataclysm-crash.log";

static GPU_LOST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The GPU device was lost (set from the device-lost callback).
pub fn set_gpu_lost() {
    GPU_LOST.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Whether the GPU device has been lost.
pub fn gpu_lost() -> bool {
    GPU_LOST.load(std::sync::atomic::Ordering::SeqCst)
}

/// Installs the hook (once, at start-up).
pub fn install() {
    #[cfg(windows)]
    native::install();
    std::panic::set_hook(Box::new(|info| {
        if gpu_lost() {
            // The GPU calls after a device loss fail; the loss itself is already in the log,
            // the failure that ends the game goes after it in a line.
            eprintln!(
                "cataclysm: the graphics device was lost (the driver reset the GPU); details are in {LOG_NAME}"
            );
            let place = info
                .location()
                .map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
            let _ = write_log(&format!(
                "cataclysm: after the device loss the game stopped{place}: {}\n",
                payload_text(info)
            ));
            if std::thread::current().name() == Some("main") {
                std::process::exit(1);
            }
        }
        let report = report(info);
        eprintln!("{report}");
        let written = write_log(&report);
        match &written {
            Some(p) => eprintln!("cataclysm: crash report written to {}", p.display()),
            None => eprintln!("cataclysm: could not write {LOG_NAME}"),
        }
        if std::thread::current().name() == Some("main") {
            let _ = std::io::stderr().flush();
            std::process::exit(101);
        }
    }));
}

/// A panic's message.
fn payload_text(info: &PanicHookInfo<'_>) -> String {
    info.payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into())
}

/// The text of a report.
fn report(info: &PanicHookInfo<'_>) -> String {
    let message = payload_text(info);
    let location = info.location().map_or_else(
        || "unknown place".into(),
        |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
    );
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("unnamed");
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!(
        "cataclysm {} crashed (unix time {since})\nthread '{name}' panicked at {location}:\n{message}\n\nbacktrace:\n{}\n",
        env!("CARGO_PKG_VERSION"),
        Backtrace::force_capture()
    )
}

/// Logs a report from outside a panic (a native failure, a GPU error).
pub fn log(what: &str) {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let thread = std::thread::current();
    let report = format!(
        "cataclysm {} (unix time {since}), thread '{}':\n{what}\n\nbacktrace:\n{}\n",
        env!("CARGO_PKG_VERSION"),
        thread.name().unwrap_or("unnamed"),
        Backtrace::force_capture()
    );
    eprintln!("{report}");
    if let Some(p) = write_log(&report) {
        eprintln!("cataclysm: report written to {}", p.display());
    }
}

/// Appends a report as it is (no backtrace of the calling thread) to the log and stderr.
pub fn append(report: &str) {
    eprintln!("{report}");
    if let Some(p) = write_log(report) {
        eprintln!("cataclysm: report written to {}", p.display());
    }
}

/// Appends the report to the first writable place; returns where it went.
fn write_log(report: &str) -> Option<PathBuf> {
    for dir in log_dirs() {
        let path = dir.join(LOG_NAME);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path);
        if let Ok(mut f) = file {
            if f.write_all(report.as_bytes()).is_ok() {
                return Some(path);
            }
        }
    }
    None
}

/// Replaces `name` in the first writable log place with `text`.
pub fn write_file(name: &str, text: &str) {
    for dir in log_dirs() {
        if std::fs::write(dir.join(name), text).is_ok() {
            return;
        }
    }
}

/// Next to the executable, then the user's config directory, then the working directory.
fn log_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
    {
        v.push(d);
    }
    if let Some(a) = std::env::var_os("APPDATA").or_else(|| std::env::var_os("XDG_CONFIG_HOME")) {
        let d = PathBuf::from(a).join("hwc-cataclysm");
        if std::fs::create_dir_all(&d).is_ok() {
            v.push(d);
        }
    }
    v.push(PathBuf::from("."));
    v
}

/// Development: the GPU object counts and the allocations by label, largest first (when the
/// backend reports them).
pub fn gpu_report(device: &wgpu::Device) {
    let h = device.get_internal_counters().hal;
    eprintln!(
        "gpu: counts: {} buffers ({:.0} MB), {} textures ({:.0} MB), {} views, {} bind groups, {} pipelines, {} encoders, {} shaders, {} allocations",
        h.buffers.read(),
        h.buffer_memory.read() as f64 / 1e6,
        h.textures.read(),
        h.texture_memory.read() as f64 / 1e6,
        h.texture_views.read(),
        h.bind_groups.read(),
        h.render_pipelines.read(),
        h.command_encoders.read(),
        h.shader_modules.read(),
        h.memory_allocations.read(),
    );
    let Some(r) = device.generate_allocator_report() else {
        eprintln!("gpu: no allocator report");
        return;
    };
    let mut by: std::collections::BTreeMap<String, (u64, usize)> = Default::default();
    for a in &r.allocations {
        let e = by.entry(a.name.clone()).or_default();
        e.0 += a.size;
        e.1 += 1;
    }
    let mut v: Vec<_> = by.into_iter().collect();
    v.sort_by_key(|e| std::cmp::Reverse(e.1 .0));
    eprintln!(
        "gpu: {:.0} MB allocated, {:.0} MB reserved",
        r.total_allocated_bytes as f64 / 1e6,
        r.total_reserved_bytes as f64 / 1e6
    );
    for (name, (size, n)) in v.iter().take(8) {
        eprintln!("gpu:   {:8.1} MB in {n:5} {name}", *size as f64 / 1e6);
    }
}

/// The Windows side: the `SIGABRT` handler and the unhandled-exception filter. The few system
/// calls are declared here (kernel32 and the C runtime) rather than pulling in a binding crate.
#[cfg(windows)]
mod native {
    use std::ffi::c_void;

    const SIGABRT: i32 = 22;
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    const STACK_OVERFLOW: u32 = 0xC000_00FD;
    const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x4;
    const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x2;

    #[repr(C)]
    struct ExceptionRecord {
        code: u32,
        flags: u32,
        next: *const ExceptionRecord,
        address: *const c_void,
        parameter_count: u32,
        information: [usize; 15],
    }

    #[repr(C)]
    struct ExceptionPointers {
        record: *const ExceptionRecord,
        context: *const c_void,
    }

    type Filter = unsafe extern "system" fn(*const ExceptionPointers) -> i32;

    unsafe extern "system" {
        fn SetUnhandledExceptionFilter(filter: Option<Filter>) -> Option<Filter>;
        fn GetModuleHandleExW(flags: u32, address: *const c_void, module: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
    }

    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }

    pub fn install() {
        // SAFETY: both register handlers with the C runtime and the system; the handlers are
        // plain functions that live as long as the program.
        unsafe {
            signal(SIGABRT, on_abort as extern "C" fn(i32) as usize);
            SetUnhandledExceptionFilter(Some(on_exception));
        }
    }

    /// `abort()` raises `SIGABRT` before the C runtime fast-fails the process.
    extern "C" fn on_abort(_sig: i32) {
        // Nothing here may unwind into the C runtime.
        let _ = std::panic::catch_unwind(|| {
            super::log("native abort() (SIGABRT): a C or C++ library (a GPU or audio driver, or std::terminate) ended the process");
        });
    }

    /// The path of the module holding `address`, if any.
    fn module_of(address: *const c_void) -> String {
        let mut module = std::ptr::null_mut();
        // SAFETY: the flags ask for the module containing `address` without changing its
        // reference count; `module` is written only on success.
        let found = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                address,
                &mut module,
            )
        };
        if found == 0 {
            return "an unknown module".into();
        }
        let mut name = [0u16; 512];
        // SAFETY: `name` has room for the length passed.
        let n =
            unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) } as usize;
        format!(
            "{} (base {module:p})",
            String::from_utf16_lossy(&name[..n.min(name.len())])
        )
    }

    /// An exception no handler took: logged, then left to the system (which ends the process).
    unsafe extern "system" fn on_exception(info: *const ExceptionPointers) -> i32 {
        // SAFETY: the system passes valid exception pointers to the filter.
        let (code, address, params) = unsafe {
            match info.as_ref().and_then(|i| i.record.as_ref()) {
                Some(r) => (
                    r.code,
                    r.address,
                    r.information[..(r.parameter_count as usize).min(15)].to_vec(),
                ),
                None => (0, std::ptr::null(), Vec::new()),
            }
        };
        if code == STACK_OVERFLOW {
            // Only the stack's small reserve is left: a line without a backtrace (the standard
            // library has printed which thread it was).
            let _ = std::panic::catch_unwind(|| {
                super::append(&format!(
                    "cataclysm {}: stack overflow (native exception 0x{code:08x}) at {address:p}\n",
                    env!("CARGO_PKG_VERSION")
                ));
            });
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let _ = std::panic::catch_unwind(|| {
            super::log(&format!(
                "native exception 0x{code:08x} at {address:p} in {}, parameters {params:x?}",
                module_of(address)
            ));
        });
        EXCEPTION_CONTINUE_SEARCH
    }
}
