//! Platform FFI helpers isolated to a single module per project rules.

use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Returns the current process's Working Set size in megabytes (MiB), or `None` on query failure.
#[must_use]
pub fn process_working_set_mib() -> Option<f64> {
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    let cb = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?;
    // SAFETY: `GetCurrentProcess()` returns a pseudo-handle to the current process, and `counters`
    // is a properly aligned, stack-allocated `PROCESS_MEMORY_COUNTERS` buffer of size `cb`.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &raw mut counters, cb) };
    if ok == 0 {
        return None;
    }
    Some(counters.WorkingSetSize as f64 / (1024.0 * 1024.0))
}
