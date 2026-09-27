// =========================================
// =========================================
// crates/motionloom/tests/formatter_cli.rs

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "motionloom-fmt-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn run(&self, arguments: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_motionloom"))
            .current_dir(&self.0)
            .args(arguments)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn check_and_write_share_library_output_and_exit_codes() {
    let fixture = Fixture::new();
    let input = "<A><B value=\"🍎\"/></A>";
    fs::write(fixture.0.join("main.motionloom"), input).unwrap();
    assert_eq!(
        fixture
            .run(&["fmt", "--check", "main.motionloom"])
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        fs::read_to_string(fixture.0.join("main.motionloom")).unwrap(),
        input
    );
    assert!(fixture.run(&["fmt", "main.motionloom"]).status.success());
    assert_eq!(
        fs::read_to_string(fixture.0.join("main.motionloom")).unwrap(),
        motionloom::api::format_dsl(input).unwrap().source
    );
    assert!(
        fixture
            .run(&["fmt", "--check", "main.motionloom"])
            .status
            .success()
    );
    assert_eq!(
        fixture
            .run(&["fmt", "--unknown", "main.motionloom"])
            .status
            .code(),
        Some(2)
    );
    assert!(fixture.run(&["--help"]).status.success());
    assert!(fixture.run(&["fmt", "--help"]).status.success());
}

#[test]
fn batch_preflight_preserves_all_files_on_syntax_error() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("nested")).unwrap();
    fs::write(fixture.0.join("first.motionloom"), "<A/>").unwrap();
    fs::write(fixture.0.join("nested/broken.motionloom"), "<A>").unwrap();
    fs::write(fixture.0.join("ignore.txt"), "<A>").unwrap();
    assert_eq!(fixture.run(&["fmt", "."]).status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(fixture.0.join("first.motionloom")).unwrap(),
        "<A/>"
    );
    assert!(fs::read_dir(&fixture.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")
    }));
}

#[cfg(unix)]
#[test]
fn recursion_skips_symlinks_and_preserves_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let path = fixture.0.join("main.motionloom");
    fs::write(&path, "<A/>").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&fixture.0, fixture.0.join("loop")).unwrap();
    assert!(fixture.run(&["fmt", "."]).status.success());
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(fixture.run(&["fmt", "loop"]).status.code(), Some(2));
}
