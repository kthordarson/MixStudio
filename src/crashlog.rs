//! Panic hook: appends message, thread, location and backtrace to `<data_dir>/crash.log`.
//! Release builds have no console (windows_subsystem), so without this panics vanish.
//! Native crashes (access violations in C/C++ deps, stack overflow, abort) bypass this hook.
use std::io::Write;
use std::path::PathBuf;

pub fn path() -> PathBuf {
    crate::library::data_dir().join("crash.log")
}

pub fn install() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let thread = std::thread::current();
        let msg = format!(
            "=== unix {secs} | MixStudio {} ===\nthread '{}' {info}\n{bt}\n",
            env!("CARGO_PKG_VERSION"),
            thread.name().unwrap_or("<unnamed>"),
        );
        let p = path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
        {
            let _ = f.write_all(msg.as_bytes());
        }
        default(info);
    }));
}
