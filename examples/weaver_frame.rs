// =========================================
// =========================================
// examples/weaver_frame.rs

// Delegate argument handling, jobs and progress to the same native CLI.
#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    let arguments = ["render", "--renderer", "weaver"].map(std::ffi::OsString::from);
    motionloom::cli::main(arguments.into_iter().chain(std::env::args_os().skip(1)))
}

#[cfg(target_arch = "wasm32")]
fn main() {}
