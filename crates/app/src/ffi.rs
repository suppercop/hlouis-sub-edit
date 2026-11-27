//! Platform FFI helpers isolated to a single module per project rules.

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

/// Process memory counters in megabytes (MiB).
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessMemoryMib {
    pub working_set_mib: f64,
    pub private_mib: f64,
}

/// Returns total user + kernel CPU time consumed by the current process in seconds.
#[must_use]
pub fn process_cpu_seconds() -> Option<f64> {
    let mut create = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: `GetCurrentProcess()` returns a pseudo-handle to the current process, and all
    // four `FILETIME` pointers point to initialized stack storage.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &raw mut create,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    };
    if ok == 0 {
        return None;
    }
    let to_u64 = |ft: FILETIME| -> u64 {
        (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
    };
    let total_100ns = to_u64(kernel).saturating_add(to_u64(user));
    Some(total_100ns as f64 / 10_000_000.0)
}

/// Returns the current process's Working Set and Private Commit sizes in MiB.
#[must_use]
pub fn process_memory_mib() -> Option<ProcessMemoryMib> {
    const MIB: f64 = 1024.0 * 1024.0;
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    let cb = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>()).ok()?;
    let ptr: *mut PROCESS_MEMORY_COUNTERS = (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>();
    // SAFETY: `GetCurrentProcess()` returns a pseudo-handle to the current process, and `counters`
    // is a properly aligned `PROCESS_MEMORY_COUNTERS_EX` buffer whose size `cb` signals the extended struct.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), ptr, cb) };
    if ok == 0 {
        return None;
    }
    Some(ProcessMemoryMib {
        working_set_mib: counters.WorkingSetSize as f64 / MIB,
        private_mib: counters.PrivateUsage as f64 / MIB,
    })
}
