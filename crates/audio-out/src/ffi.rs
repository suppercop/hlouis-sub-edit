//! The only `unsafe` in this crate: process CPU accounting for latency / CPU reports.

use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread, GetProcessTimes};
use windows_sys::Win32::System::WindowsProgramming::{QueryProcessCycleTime, QueryThreadCycleTime};

const fn filetime_100ns(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// Total user + kernel CPU time consumed by this process so far (scheduler-tick accounting,
/// ~15.6 ms granularity; prefer [`process_cpu_cycles`] for short or bursty measurements).
#[must_use]
pub fn process_cpu_time() -> Option<Duration> {
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that is always valid for the current
    // process and needs no closing; the four out-pointers reference live, writable FILETIME
    // locals for the duration of the call.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    };
    if ok == 0 {
        return None;
    }
    let total_100ns = filetime_100ns(kernel) + filetime_100ns(user);
    Some(Duration::from_nanos(total_100ns.saturating_mul(100)))
}

/// CPU cycles consumed by all threads of this process so far (precise, not tick-sampled).
#[must_use]
pub fn process_cpu_cycles() -> Option<u64> {
    let mut cycles = 0u64;
    // SAFETY: pseudo-handle for the current process (always valid, never closed); `cycles` is a
    // live, writable local.
    let ok = unsafe { QueryProcessCycleTime(GetCurrentProcess(), &raw mut cycles) };
    (ok != 0).then_some(cycles)
}

fn thread_cpu_cycles() -> Option<u64> {
    let mut cycles = 0u64;
    // SAFETY: pseudo-handle for the current thread (always valid, never closed); `cycles` is a
    // live, writable local.
    let ok = unsafe { QueryThreadCycleTime(GetCurrentThread(), &raw mut cycles) };
    (ok != 0).then_some(cycles)
}

/// Estimates the cycle-counter rate used by [`process_cpu_cycles`] by spinning the calling
/// thread for `spin` (cycles counted while running / wall time). Used to turn cycles into
/// "% of one core".
#[must_use]
pub fn cycles_per_second(spin: Duration) -> Option<f64> {
    let c0 = thread_cpu_cycles()?;
    let t0 = Instant::now();
    while t0.elapsed() < spin {
        std::hint::spin_loop();
    }
    let c1 = thread_cpu_cycles()?;
    let secs = t0.elapsed().as_secs_f64();
    (secs > 0.0).then(|| (c1 - c0) as f64 / secs)
}
