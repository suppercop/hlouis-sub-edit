//! Small Win32 helpers: thread priority for background indexing, process CPU time for benches.

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetProcessTimes, SetThreadPriority,
    THREAD_PRIORITY_BELOW_NORMAL, THREAD_PRIORITY_IDLE,
};

/// Scheduling class for a background worker thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadPriority {
    BelowNormal,
    Idle,
}

/// Lowers the calling thread's priority. Returns `false` if Windows refused.
///
/// Note: threads that `FFmpeg` spawns for frame/slice decoding do not inherit this; their count is
/// bounded separately (see `background_decode_threads`).
#[must_use]
pub fn set_current_thread_priority(p: ThreadPriority) -> bool {
    let level = match p {
        ThreadPriority::BelowNormal => THREAD_PRIORITY_BELOW_NORMAL,
        ThreadPriority::Idle => THREAD_PRIORITY_IDLE,
    };
    // SAFETY: `GetCurrentThread` returns a pseudo-handle that is always valid for the caller and
    // needs no closing; `SetThreadPriority` only reads it.
    unsafe { SetThreadPriority(GetCurrentThread(), level) != 0 }
}

fn filetime_100ns(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// Total user + kernel CPU time consumed by this process so far.
#[must_use]
pub fn process_cpu_time() -> std::time::Duration {
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut c, mut e, mut k, mut u) = (zero, zero, zero, zero);
    // SAFETY: the pseudo-handle from `GetCurrentProcess` is always valid; the four out pointers
    // reference live stack `FILETIME`s.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &raw mut c,
            &raw mut e,
            &raw mut k,
            &raw mut u,
        )
    };
    if ok == 0 {
        return std::time::Duration::ZERO;
    }
    std::time::Duration::from_nanos((filetime_100ns(k) + filetime_100ns(u)) * 100)
}
