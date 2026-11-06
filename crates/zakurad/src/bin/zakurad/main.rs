//! Main entry point for zakurad

use zakurad::application::boot;

#[cfg(all(target_os = "linux", target_env = "gnu"))]
const GLIBC_MALLOC_ARENA_MAX: i32 = 16;

/// How often freed glibc heap pages are returned to the OS.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const GLIBC_MALLOC_TRIM_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(
    unsafe_code,
    reason = "calling glibc mallopt is required to bound allocator arena retention"
)]
fn limit_glibc_malloc_arenas() {
    if std::env::var_os("MALLOC_ARENA_MAX").is_some() {
        return;
    }

    // SAFETY: this process-global allocator setting is configured before worker threads start.
    let configured = unsafe { libc::mallopt(libc::M_ARENA_MAX, GLIBC_MALLOC_ARENA_MAX) };

    assert_ne!(
        configured, 0,
        "glibc accepts M_ARENA_MAX before worker threads start"
    );
}

/// Periodically returns freed glibc heap pages to the OS.
///
/// glibc keeps freed memory in its arenas, so a burst of large reads (such as explorer requests
/// that decode many historical transactions) leaves RSS near its peak long after the memory is
/// free. Trimming lets the next burst start from the working set instead of the previous peak.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(
    unsafe_code,
    reason = "calling glibc malloc_trim is required to release freed heap pages"
)]
fn spawn_glibc_malloc_trimmer() {
    std::thread::Builder::new()
        .name("malloc-trim".to_owned())
        .spawn(|| loop {
            std::thread::sleep(GLIBC_MALLOC_TRIM_INTERVAL);
            // SAFETY: malloc_trim only releases free memory and takes glibc's arena locks.
            unsafe { libc::malloc_trim(0) };
        })
        .expect("the malloc trim thread can be spawned");
}

#[cfg(target_arch = "x86_64")]
mod cpu_features;

/// Process entry point for `zakurad`
fn main() {
    #[cfg(target_arch = "x86_64")]
    cpu_features::exit_if_cpu_lacks_pasta_assembly_features();

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        limit_glibc_malloc_arenas();
        spawn_glibc_malloc_trimmer();
    }

    // Enable backtraces by default for zakurad, but allow users to override it.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
        // Disable library backtraces (i.e. eyre) to avoid performance hit for
        // non-panic errors, but allow users to override it.
        if std::env::var_os("RUST_LIB_BACKTRACE").is_none() {
            std::env::set_var("RUST_LIB_BACKTRACE", "0");
        }
    }
    boot();
}
