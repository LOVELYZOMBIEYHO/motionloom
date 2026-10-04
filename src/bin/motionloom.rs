// =========================================
// =========================================
// src/bin/motionloom.rs

// Keep command behavior shared with the native example entry points.
#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    motionloom::cli::main(std::env::args_os().skip(1))
}

#[cfg(target_arch = "wasm32")]
fn main() {}
