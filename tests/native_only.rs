use std::process::Command;

fn typhon(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_oblivion-one"))
        .args(args)
        .output()
        .expect("Typhon CLI should execute")
}

#[test]
fn removed_nested_command_is_rejected() {
    let output = typhon(&["nested"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown command"));
}

#[test]
fn removed_prototype_command_is_rejected() {
    let output = typhon(&["prototype"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown command"));
}

#[test]
fn removed_compositor_backend_options_are_rejected() {
    for option in [
        "--output=nested",
        "--output=auto",
        "--renderer=gpu",
        "--mode",
        "--backend",
        "--windowed",
        "--host-wayland",
    ] {
        let output = typhon(&["compositor", option]);

        assert!(!output.status.success(), "{option}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unknown compositor option"),
            "{option}"
        );
    }
}
