//! Program arguments and environment, read from the initial stack.

use core::cell::UnsafeCell;
use core::ffi::CStr;

struct Startup {
    argc: usize,
    argv: *const *const u8,
    envp: *const *const u8,
}

struct Cell(UnsafeCell<Startup>);
// SAFETY: written once in `init` before `main` runs; programs are
// single-threaded.
unsafe impl Sync for Cell {}

static STARTUP: Cell = Cell(UnsafeCell::new(Startup {
    argc: 0,
    argv: core::ptr::null(),
    envp: core::ptr::null(),
}));

/// # Safety
/// `sp` must point at the kernel-built initial stack.
pub(crate) unsafe fn init(sp: *const u64) {
    unsafe {
        let argc = *sp as usize;
        let argv = sp.add(1) as *const *const u8;
        let envp = argv.add(argc + 1);
        *STARTUP.0.get() = Startup { argc, argv, envp };
    }
}

fn startup() -> &'static Startup {
    // SAFETY: only written by `init`.
    unsafe { &*STARTUP.0.get() }
}

fn to_str(p: *const u8) -> &'static str {
    // SAFETY: the kernel places NUL-terminated UTF-8 strings on the stack.
    unsafe { CStr::from_ptr(p.cast()).to_str().unwrap_or("") }
}

/// Program arguments, `args().get(0)` is the program path.
#[derive(Clone, Copy)]
pub struct Args;

impl Args {
    pub fn len(&self) -> usize {
        startup().argc
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, i: usize) -> Option<&'static str> {
        let s = startup();
        // SAFETY: argv has argc valid entries.
        (i < s.argc).then(|| to_str(unsafe { *s.argv.add(i) }))
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static str> {
        (0..self.len()).filter_map(|i| self.get(i))
    }
}

pub fn args() -> Args {
    Args
}

/// All "NAME=value" environment entries.
pub fn vars() -> impl Iterator<Item = &'static str> {
    let mut p = startup().envp;
    core::iter::from_fn(move || {
        if p.is_null() {
            return None;
        }
        // SAFETY: envp is a NULL-terminated array.
        let s = unsafe { *p };
        if s.is_null() {
            return None;
        }
        // SAFETY: moving to the next entry of the same array.
        p = unsafe { p.add(1) };
        Some(to_str(s))
    })
}

/// Value of the environment variable `name`.
pub fn var(name: &str) -> Option<&'static str> {
    vars().find_map(|kv| {
        kv.strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
    })
}
