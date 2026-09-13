use std::collections::BTreeMap;

use oblivion_one::native::dmem_foreground::{
    DmemForegroundPolicy, DmemLowWriter, RelativeCgroupPath, apply_low_entries, parse_capacity,
    parse_cgroup_v2_path, validate_cgroup_target,
};

#[derive(Default)]
struct RecordingWriter {
    writes: Vec<(String, String, u64)>,
}

impl DmemLowWriter for RecordingWriter {
    fn write_region(
        &mut self,
        cgroup: &RelativeCgroupPath,
        region: &str,
        value: u64,
    ) -> std::io::Result<()> {
        self.writes.push((
            cgroup.as_path().display().to_string(),
            region.to_owned(),
            value,
        ));
        Ok(())
    }
}

#[test]
fn capacity_and_nested_key_writes_are_deterministic() {
    let capacities = parse_capacity("gpu-b 2\ngpu-a 1\n").expect("capacity");
    assert_eq!(
        capacities,
        BTreeMap::from([("gpu-a".to_owned(), 1), ("gpu-b".to_owned(), 2),])
    );

    let cgroup = RelativeCgroupPath::parse("/user.slice/app.scope").expect("cgroup");
    let mut writer = RecordingWriter::default();
    apply_low_entries(&mut writer, &cgroup, &capacities).expect("writes");
    assert_eq!(
        writer.writes,
        vec![
            ("user.slice/app.scope".to_owned(), "gpu-a".to_owned(), 1),
            ("user.slice/app.scope".to_owned(), "gpu-b".to_owned(), 2),
        ]
    );
}

#[test]
fn cgroup_identity_is_unified_and_contained() {
    let target = parse_cgroup_v2_path("1:name=systemd:/system.slice\n0::/user.slice/app.scope\n")
        .expect("unified entry");
    let own = RelativeCgroupPath::parse("/user.slice/typhon.scope").expect("own cgroup");
    assert!(validate_cgroup_target(&target, &own).is_ok());

    let ancestor = RelativeCgroupPath::parse("/user.slice").expect("ancestor");
    assert!(validate_cgroup_target(&ancestor, &own).is_err());
    assert!(parse_cgroup_v2_path("0::/user.slice/../other.scope\n").is_err());
}

#[test]
fn policy_accepts_only_the_documented_modes() {
    assert_eq!(
        DmemForegroundPolicy::parse("auto"),
        DmemForegroundPolicy::Auto
    );
    assert_eq!(DmemForegroundPolicy::parse("ON"), DmemForegroundPolicy::On);
    assert_eq!(
        DmemForegroundPolicy::parse("off"),
        DmemForegroundPolicy::Off
    );
    assert_eq!(
        DmemForegroundPolicy::parse("unknown"),
        DmemForegroundPolicy::Auto
    );
}
