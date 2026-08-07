use crate::compositor::astrea_shell_capability::AstreaShellCapability;
use std::os::unix::fs::PermissionsExt;

#[test]
fn astrea_shell_capability_is_bounded_protected_redacted_and_rotates() {
    let directory = std::env::temp_dir().join(format!(
        "oblivion-one-capability-test-{}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("capability");

    let first = AstreaShellCapability::create_for_path(&path).unwrap();
    let first_verifier = first.verifier();
    let first_value = std::fs::read_to_string(&path).unwrap();
    assert_eq!(first_value.len(), 65);
    assert!(first_value.ends_with('\n'));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!format!("{:?}", first).contains(first_value.trim()));

    drop(first);
    let second = AstreaShellCapability::create_for_path(&path).unwrap();
    let second_value = std::fs::read_to_string(&path).unwrap();
    assert_ne!(first_value, second_value);
    assert!(!first_verifier.matches(second_value.trim()));
    assert!(!format!("{:?}", second).contains(second_value.trim()));

    drop(second);
    assert!(!path.exists());
    std::fs::remove_dir(&directory).unwrap();
}
