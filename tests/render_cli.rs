// =========================================
// =========================================
// crates/motionloom/tests/render_cli.rs

use std::process::Command;

fn run(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_motionloom"))
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn command_help_is_available_without_starting_a_renderer() {
    for command in ["render", "export"] {
        let output = run(&[command, "--help"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let help = String::from_utf8_lossy(&output.stdout);
        assert!(help.contains("--renderer weaver"));
        assert!(help.contains("Ctrl+C"));
        assert!(help.contains(if command == "render" {
            "--frame N"
        } else {
            "--frames START:END"
        }));
    }
    let output = run(&["--help"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("motionloom fmt"));
}

#[test]
fn invalid_arguments_fail_before_any_gpu_or_output_work() {
    let cases: &[(&[&str], &str)] = &[
        (&["render", "main.motionloom"], "Specify --renderer"),
        (
            &["render", "main.motionloom", "--renderer", "wgpu"],
            "Unsupported renderer",
        ),
        (
            &[
                "render",
                "main.motionloom",
                "--renderer",
                "weaver",
                "--samples",
            ],
            "Missing value",
        ),
        (
            &[
                "render",
                "main.motionloom",
                "--renderer",
                "weaver",
                "--samples",
                "0",
            ],
            "--samples",
        ),
        (
            &[
                "render",
                "main.motionloom",
                "--renderer",
                "weaver",
                "--frame",
                "-1",
            ],
            "--frame",
        ),
        (
            &[
                "render",
                "main.motionloom",
                "--renderer",
                "weaver",
                "--size",
                "0x64",
            ],
            "--size",
        ),
        (
            &[
                "render",
                "main.motionloom",
                "--renderer",
                "weaver",
                "--frames",
                "0:1",
            ],
            "Unknown option",
        ),
        (
            &[
                "export",
                "main.motionloom",
                "--renderer",
                "weaver",
                "--frame",
                "1",
            ],
            "Unknown option",
        ),
        (
            &[
                "export",
                "main.motionloom",
                "--renderer",
                "weaver",
                "--frames",
                "2:1",
            ],
            "START",
        ),
        (
            &[
                "export",
                "main.motionloom",
                "--renderer",
                "weaver",
                "--frames",
                "bad",
            ],
            "START:END",
        ),
        (
            &[
                "render",
                "main.motionloom",
                "--renderer",
                "weaver",
                "--dof",
                "--no-dof",
            ],
            "conflict",
        ),
    ];
    for (args, message) in cases {
        let output = run(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
    }
}

#[cfg(not(feature = "weaver"))]
#[test]
fn disabled_weaver_has_an_actionable_build_message() {
    let output = run(&["render", "main.motionloom", "--renderer", "weaver"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--features weaver"));
}

#[cfg(feature = "weaver")]
#[test]
fn timeline_and_optics_are_validated_before_allocating_a_gpu() {
    let scene = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cli-render.motionloom"
    );
    for (flags, message) in [
        (vec!["--frame", "999"], "outside the DSL timeline"),
        (vec!["--f-stop", "NaN"], "lens override"),
    ] {
        let mut args = vec!["render", scene, "--renderer", "weaver"];
        args.extend(flags);
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
