use super::*;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[test]
fn capacity_parser_accepts_multiple_regions_in_sorted_order() {
    let capacity =
        parse_capacity("region-b 67108864\nregion-a 8589934592\n").expect("capacity should parse");

    assert_eq!(
        capacity,
        [
            ("region-a".to_owned(), 8_589_934_592),
            ("region-b".to_owned(), 67_108_864),
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn capacity_parser_rejects_malformed_duplicate_and_overflow_entries() {
    for input in [
        "region-a",
        "region-a 1 extra",
        "region-a nope",
        "region-a 18446744073709551616",
        "region-a 1\nregion-a 2",
    ] {
        assert!(
            parse_capacity(input).is_err(),
            "input should fail: {input:?}"
        );
    }

    assert!(parse_capacity("\n  \n").is_err());
}

#[test]
fn policy_parser_defaults_and_normalizes_unknown_values_to_auto() {
    assert_eq!(
        DmemForegroundPolicy::parse("auto"),
        DmemForegroundPolicy::Auto
    );
    assert_eq!(
        DmemForegroundPolicy::parse(" ON "),
        DmemForegroundPolicy::On
    );
    assert_eq!(
        DmemForegroundPolicy::parse("off"),
        DmemForegroundPolicy::Off
    );
    assert_eq!(
        DmemForegroundPolicy::parse("unexpected"),
        DmemForegroundPolicy::Auto
    );
    assert_eq!(DmemForegroundPolicy::parse(""), DmemForegroundPolicy::Auto);
}

#[test]
fn cgroup_parser_accepts_unified_entry_and_rejects_unsafe_paths() {
    assert_eq!(
        parse_cgroup_v2_path("5:cpu:/legacy\n0::/user.slice/app.slice/foo.scope\n")
            .expect("unified cgroup should parse")
            .as_path(),
        Path::new("user.slice/app.slice/foo.scope")
    );

    for input in [
        "5:cpu:/legacy\n",
        "0::/user.slice/../other.scope\n",
        "0::/user.slice/./foo.scope\n",
        "0::/user.slice/foo.scope (deleted)\n",
        "0:/wrong-field-count\n",
    ] {
        assert!(
            parse_cgroup_v2_path(input).is_err(),
            "input should fail: {input:?}"
        );
    }
}

#[test]
fn process_metadata_parsers_require_valid_uid_and_start_time() {
    assert_eq!(
        parse_process_uid("Name:\tapp\nUid:\t1000\t1000\t1000\t1000\n").expect("uid should parse"),
        1000
    );
    assert!(parse_process_uid("Name:\tapp\n").is_err());
    assert!(parse_process_uid("Uid:\t1000\tnope\n").is_err());

    let stat = "123 (app process) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21";
    assert_eq!(
        parse_process_start_time(stat).expect("start time should parse"),
        19
    );
    assert!(parse_process_start_time("123 (app) S").is_err());
}

#[test]
fn pid_reuse_with_changed_start_time_is_rejected_before_mutation() {
    let paths = resolver_fixture("pid-reuse", 77, 1000, "/user.slice/app.scope");
    let own = RelativeCgroupPath::parse("/user.slice/typhon.scope").expect("own path");
    let initial = "123 (fixture) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21";
    let reused = "123 (fixture) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 22 20 21";
    let resolver = ProcCgroupResolver::new(paths.clone(), 1000, own)
        .with_start_time_sequence(vec![initial.to_owned(), reused.to_owned()]);

    assert!(matches!(
        resolver.resolve_pid(77),
        Err(DmemError::StaleProcessIdentity)
    ));
    std::fs::remove_dir_all(paths.proc_root.parent().expect("fixture root")).expect("cleanup");
}

#[test]
fn target_validation_rejects_root_own_cgroup_and_unsafe_ancestors() {
    let own = RelativeCgroupPath::parse("/user.slice/user-1000.slice/session.scope")
        .expect("own path should parse");

    assert!(
        validate_cgroup_target(
            &RelativeCgroupPath::parse("/user.slice/app.scope").expect("target should parse"),
            &own,
        )
        .is_ok()
    );
    for target in [
        "/",
        "/user.slice/user-1000.slice/session.scope",
        "/user.slice",
        "/user.slice/user-1000.slice",
    ] {
        assert!(
            validate_cgroup_target(&RelativeCgroupPath::parse(target).expect("path"), &own)
                .is_err(),
            "target should fail: {target}"
        );
    }
}

#[derive(Default)]
struct RecordingWriter {
    writes: Vec<(String, String, u64)>,
    missing: bool,
}

impl DmemLowWriter for RecordingWriter {
    fn write_region(
        &mut self,
        cgroup: &RelativeCgroupPath,
        region: &str,
        value: u64,
    ) -> std::io::Result<()> {
        if self.missing {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        }
        self.writes.push((
            cgroup.as_path().display().to_string(),
            region.to_owned(),
            value,
        ));
        Ok(())
    }
}

#[test]
fn low_entries_are_written_as_distinct_region_value_operations() {
    let mut writer = RecordingWriter::default();
    let cgroup = RelativeCgroupPath::parse("/user.slice/app.scope").expect("path");
    let capacities = parse_capacity("region-b 2\nregion-a 1\n").expect("capacity");

    apply_low_entries(&mut writer, &cgroup, &capacities).expect("apply should succeed");
    assert_eq!(
        writer.writes,
        vec![
            ("user.slice/app.scope".to_owned(), "region-a".to_owned(), 1),
            ("user.slice/app.scope".to_owned(), "region-b".to_owned(), 2),
        ]
    );

    writer.writes.clear();
    revert_low_entries(&mut writer, &cgroup, capacities.keys()).expect("revert should succeed");
    assert_eq!(
        writer.writes,
        vec![
            ("user.slice/app.scope".to_owned(), "region-a".to_owned(), 0),
            ("user.slice/app.scope".to_owned(), "region-b".to_owned(), 0),
        ]
    );
}

#[derive(Default)]
struct RawWriteProbe {
    calls: Vec<Vec<u8>>,
    outcomes: VecDeque<std::io::Result<usize>>,
}

impl RawWrite for RawWriteProbe {
    fn write(&mut self, _fd: std::os::fd::RawFd, record: &[u8]) -> std::io::Result<usize> {
        self.calls.push(record.to_vec());
        self.outcomes.pop_front().unwrap_or(Ok(record.len()))
    }
}

#[test]
fn raw_keyed_write_sends_one_complete_capacity_record() {
    let mut writer = RawWriteProbe::default();
    write_region_with_raw(&mut writer, -1, "drm/0000:01:00.0/vram", 8_589_934_592)
        .expect("raw write");

    assert_eq!(
        writer.calls,
        vec![b"drm/0000:01:00.0/vram 8589934592\n".to_vec()]
    );
}

#[test]
fn raw_keyed_write_uses_one_complete_zero_record_for_revert() {
    let mut writer = RawWriteProbe::default();
    write_region_with_raw(&mut writer, -1, "drm/0000:01:00.0/vram", 0).expect("raw write");

    assert_eq!(writer.calls, vec![b"drm/0000:01:00.0/vram 0\n".to_vec()]);
}

#[test]
fn raw_keyed_write_rejects_positive_short_write_without_suffix_retry() {
    let mut writer = RawWriteProbe {
        outcomes: VecDeque::from([Ok(3)]),
        ..RawWriteProbe::default()
    };
    let record = build_low_record("region", 10).expect("record");

    assert!(write_single_record_with(&mut writer, -1, &record).is_err());
    assert_eq!(writer.calls, vec![b"region 10\n".to_vec()]);
}

#[test]
fn raw_keyed_write_retries_eintr_with_the_complete_record() {
    let mut writer = RawWriteProbe {
        outcomes: VecDeque::from([
            Err(std::io::Error::from(std::io::ErrorKind::Interrupted)),
            Ok(10),
        ]),
        ..RawWriteProbe::default()
    };
    let record = build_low_record("region", 10).expect("record");

    write_single_record_with(&mut writer, -1, &record).expect("retry write");
    assert_eq!(
        writer.calls,
        vec![b"region 10\n".to_vec(), b"region 10\n".to_vec()]
    );
}

#[test]
fn malformed_keyed_record_is_rejected_before_the_raw_write() {
    let mut writer = RawWriteProbe::default();
    for region in ["", "region\nother", "region\0other"] {
        assert!(
            write_region_with_raw(&mut writer, -1, region, 10).is_err(),
            "region should fail: {region:?}"
        );
    }
    assert!(writer.calls.is_empty());
}

#[derive(Default)]
struct FailOnWriteWriter {
    attempts: Vec<(String, String, u64)>,
    successful_writes: Vec<(String, String, u64)>,
    fail_on: Option<usize>,
}

impl DmemLowWriter for FailOnWriteWriter {
    fn write_region(
        &mut self,
        cgroup: &RelativeCgroupPath,
        region: &str,
        value: u64,
    ) -> std::io::Result<()> {
        let operation = (
            cgroup.as_path().display().to_string(),
            region.to_owned(),
            value,
        );
        self.attempts.push(operation.clone());
        if self.fail_on == Some(self.attempts.len()) {
            return Err(std::io::Error::other("injected dmem write failure"));
        }
        self.successful_writes.push(operation);
        Ok(())
    }
}

#[test]
fn partial_capacity_apply_rolls_back_regions_and_publishes_no_current_target() {
    let resolved = RecordingWriter::path("/user.slice/app.scope", 42);
    let resolver = StubResolver {
        results: [(42, Ok(resolved))].into_iter().collect(),
    };
    let mut controller = ForegroundController::new(
        resolver,
        FailOnWriteWriter {
            fail_on: Some(2),
            ..FailOnWriteWriter::default()
        },
        parse_capacity("region-a 10\nregion-b 20").expect("capacity"),
    );
    let target = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(1).expect("window id"),
        pid: 42,
    };

    assert!(controller.reconcile(Some(target)).is_err());
    assert_eq!(controller.current_path(), None);
    assert_eq!(
        controller.writer().attempts,
        vec![
            ("user.slice/app.scope".to_owned(), "region-a".to_owned(), 10),
            ("user.slice/app.scope".to_owned(), "region-b".to_owned(), 20),
            ("user.slice/app.scope".to_owned(), "region-a".to_owned(), 0),
            ("user.slice/app.scope".to_owned(), "region-b".to_owned(), 0),
        ]
    );
}

#[test]
fn cleanup_attempts_every_region_after_one_revert_write_fails() {
    let resolved = RecordingWriter::path("/user.slice/app.scope", 42);
    let resolver = StubResolver {
        results: [(42, Ok(resolved))].into_iter().collect(),
    };
    let mut controller = ForegroundController::new(
        resolver,
        FailOnWriteWriter {
            fail_on: Some(3),
            ..FailOnWriteWriter::default()
        },
        parse_capacity("region-a 10\nregion-b 20").expect("capacity"),
    );
    let target = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(1).expect("window id"),
        pid: 42,
    };
    controller.reconcile(Some(target)).expect("apply");

    assert!(controller.reconcile(None).is_err());
    assert_eq!(
        controller
            .writer()
            .attempts
            .iter()
            .skip(2)
            .map(|(_, region, value)| (region.as_str(), *value))
            .collect::<Vec<_>>(),
        vec![("region-a", 0), ("region-b", 0)]
    );
}

struct StaleAfterFirstWrite {
    writes: Vec<(String, String, u64)>,
    stale: Arc<AtomicBool>,
}

impl DmemLowWriter for StaleAfterFirstWrite {
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
        if value != 0 {
            self.stale.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[test]
fn stale_generation_stops_between_regions_and_reverts_touched_entries() {
    let resolved = RecordingWriter::path("/user.slice/app.scope", 42);
    let resolver = StubResolver {
        results: [(42, Ok(resolved))].into_iter().collect(),
    };
    let stale = Arc::new(AtomicBool::new(false));
    let mut controller = ForegroundController::new(
        resolver,
        StaleAfterFirstWrite {
            writes: Vec::new(),
            stale: Arc::clone(&stale),
        },
        parse_capacity("region-a 10\nregion-b 20").expect("capacity"),
    );
    let target = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(1).expect("window id"),
        pid: 42,
    };

    assert_eq!(
        controller
            .reconcile_with(Some(target), || !stale.load(Ordering::SeqCst))
            .expect("stale transition cleanup"),
        TransitionResult::Stale
    );
    assert_eq!(controller.current_path(), None);
    assert_eq!(
        controller.writer().writes,
        vec![
            ("user.slice/app.scope".to_owned(), "region-a".to_owned(), 10),
            ("user.slice/app.scope".to_owned(), "region-a".to_owned(), 0),
        ]
    );
}

#[test]
fn stale_generation_before_revert_keeps_the_current_target_protected() {
    let first = RecordingWriter::path("/user.slice/app-a.scope", 42);
    let second = RecordingWriter::path("/user.slice/app-b.scope", 43);
    let resolver = StubResolver {
        results: [(42, Ok(first.clone())), (43, Ok(second))]
            .into_iter()
            .collect(),
    };
    let mut controller = controller_fixture(resolver);
    let target_a = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(1).expect("window id"),
        pid: 42,
    };
    let target_b = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(2).expect("window id"),
        pid: 43,
    };

    controller.reconcile(Some(target_a)).expect("first apply");
    let writes_before = controller.writer().writes.clone();

    assert_eq!(
        controller
            .reconcile_with(Some(target_b), || false)
            .expect("stale transition"),
        TransitionResult::Stale
    );
    assert_eq!(controller.current_path(), Some(first.path.as_path()));
    assert_eq!(controller.writer().writes, writes_before);
}

fn resolver_fixture(name: &str, pid: u32, uid: u32, cgroup: &str) -> DmemPaths {
    let root =
        std::env::temp_dir().join(format!("typhon-dmem-{}-{name}-{}", std::process::id(), pid));
    let proc_dir = root.join("proc").join(pid.to_string());
    let cgroup_dir = root.join("cgroup").join(cgroup.trim_start_matches('/'));
    std::fs::create_dir_all(&proc_dir).expect("proc fixture");
    std::fs::create_dir_all(&cgroup_dir).expect("cgroup fixture");
    std::fs::write(
        proc_dir.join("status"),
        format!("Name:\tfixture\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
    )
    .expect("status fixture");
    std::fs::write(
        proc_dir.join("stat"),
        "123 (fixture) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21",
    )
    .expect("stat fixture");
    std::fs::write(proc_dir.join("cgroup"), format!("0::{cgroup}\n"))
        .expect("cgroup membership fixture");
    std::fs::write(cgroup_dir.join("dmem.low"), "").expect("dmem.low fixture");
    DmemPaths {
        proc_root: root.join("proc"),
        cgroup_root: root.join("cgroup"),
    }
}

#[test]
fn resolver_accepts_same_uid_and_rejects_different_uid() {
    let accepted_paths = resolver_fixture("same-uid", 42, 1000, "/user.slice/app.scope");
    let own = RelativeCgroupPath::parse("/user.slice/typhon.scope").expect("own path");
    let resolver = ProcCgroupResolver::new(accepted_paths.clone(), 1000, own.clone());
    assert!(resolver.resolve_pid(42).is_ok());
    std::fs::remove_dir_all(accepted_paths.proc_root.parent().expect("fixture root"))
        .expect("cleanup");

    let rejected_paths = resolver_fixture("different-uid", 43, 1001, "/user.slice/app.scope");
    let resolver = ProcCgroupResolver::new(rejected_paths.clone(), 1000, own);
    assert!(matches!(
        resolver.resolve_pid(43),
        Err(DmemError::InvalidTarget("different uid"))
    ));
    std::fs::remove_dir_all(rejected_paths.proc_root.parent().expect("fixture root"))
        .expect("cleanup");
}

#[test]
fn resolver_rejects_missing_or_unusable_target_cgroup() {
    let paths = resolver_fixture("missing-low", 44, 1000, "/user.slice/app.scope");
    std::fs::remove_file(paths.cgroup_root.join("user.slice/app.scope/dmem.low"))
        .expect("remove dmem.low");
    let own = RelativeCgroupPath::parse("/user.slice/typhon.scope").expect("own path");
    let resolver = ProcCgroupResolver::new(paths.clone(), 1000, own);
    assert!(resolver.resolve_pid(44).is_err());
    assert!(resolver.resolve_pid(45).is_err());
    std::fs::remove_dir_all(paths.proc_root.parent().expect("fixture root")).expect("cleanup");
}

#[derive(Clone)]
struct StubResolver {
    results: BTreeMap<u32, Result<ResolvedCgroup, DmemError>>,
}

impl DmemTargetResolver for StubResolver {
    fn resolve_pid(&self, pid: u32) -> Result<ResolvedCgroup, DmemError> {
        self.results
            .get(&pid)
            .cloned()
            .unwrap_or(Err(DmemError::InvalidTarget("unknown test pid")))
    }
}

impl RecordingWriter {
    fn path(path: &str, pid: u32) -> ResolvedCgroup {
        ResolvedCgroup {
            path: RelativeCgroupPath::parse(path).expect("path"),
            process: ProcessIdentity { pid, start_time: 1 },
        }
    }
}

fn controller_fixture(
    resolver: StubResolver,
) -> ForegroundController<StubResolver, RecordingWriter> {
    ForegroundController::new(
        resolver,
        RecordingWriter::default(),
        parse_capacity("region-a 10\nregion-b 20").expect("capacity"),
    )
}

#[test]
fn controller_applies_and_reverts_all_regions_for_a_target() {
    let resolved = RecordingWriter::path("/user.slice/app-a.scope", 42);
    let resolver = StubResolver {
        results: [(42, Ok(resolved.clone()))].into_iter().collect(),
    };
    let mut controller = controller_fixture(resolver);
    let target = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(1).expect("window id"),
        pid: 42,
    };

    assert_eq!(
        controller.reconcile(Some(target)).expect("apply"),
        TransitionResult::Applied
    );
    assert_eq!(controller.current_path(), Some(resolved.path.as_path()));

    assert_eq!(
        controller.reconcile(None).expect("revert"),
        TransitionResult::Reverted
    );
    assert_eq!(controller.current_path(), None);
    assert_eq!(
        controller.writer().writes,
        vec![
            (
                "user.slice/app-a.scope".to_owned(),
                "region-a".to_owned(),
                10
            ),
            (
                "user.slice/app-a.scope".to_owned(),
                "region-b".to_owned(),
                20
            ),
            (
                "user.slice/app-a.scope".to_owned(),
                "region-a".to_owned(),
                0
            ),
            (
                "user.slice/app-a.scope".to_owned(),
                "region-b".to_owned(),
                0
            ),
        ]
    );
}

#[test]
fn controller_reverts_old_target_before_applying_a_different_target() {
    let first = RecordingWriter::path("/user.slice/app-a.scope", 42);
    let second = RecordingWriter::path("/user.slice/app-b.scope", 43);
    let resolver = StubResolver {
        results: [(42, Ok(first.clone())), (43, Ok(second.clone()))]
            .into_iter()
            .collect(),
    };
    let mut controller = controller_fixture(resolver);
    let window_a = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(1).expect("window id"),
        pid: 42,
    };
    let window_b = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(2).expect("window id"),
        pid: 43,
    };

    controller.reconcile(Some(window_a)).expect("first apply");
    assert_eq!(
        controller.reconcile(Some(window_b)).expect("second apply"),
        TransitionResult::Applied
    );
    assert_eq!(
        controller.writer().writes,
        vec![
            (
                "user.slice/app-a.scope".to_owned(),
                "region-a".to_owned(),
                10
            ),
            (
                "user.slice/app-a.scope".to_owned(),
                "region-b".to_owned(),
                20
            ),
            (
                "user.slice/app-a.scope".to_owned(),
                "region-a".to_owned(),
                0
            ),
            (
                "user.slice/app-a.scope".to_owned(),
                "region-b".to_owned(),
                0
            ),
            (
                "user.slice/app-b.scope".to_owned(),
                "region-a".to_owned(),
                10
            ),
            (
                "user.slice/app-b.scope".to_owned(),
                "region-b".to_owned(),
                20
            ),
        ]
    );
}

#[test]
fn controller_skips_writes_for_same_cgroup_and_cleans_up_failed_new_targets() {
    let first = RecordingWriter::path("/user.slice/app.scope", 42);
    let resolver = StubResolver {
        results: [
            (42, Ok(first.clone())),
            (43, Ok(RecordingWriter::path("/user.slice/app.scope", 43))),
            (44, Err(DmemError::InvalidTarget("new target unavailable"))),
        ]
        .into_iter()
        .collect(),
    };
    let mut controller = controller_fixture(resolver);
    let target_a = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(1).expect("window id"),
        pid: 42,
    };
    let target_same_cgroup = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(2).expect("window id"),
        pid: 43,
    };
    let target_failed = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(3).expect("window id"),
        pid: 44,
    };

    controller.reconcile(Some(target_a)).expect("first apply");
    assert_eq!(
        controller
            .reconcile(Some(target_same_cgroup))
            .expect("same cgroup"),
        TransitionResult::SameCgroup
    );
    assert_eq!(controller.writer().writes.len(), 2);
    assert!(controller.reconcile(Some(target_failed)).is_err());
    assert_eq!(controller.current_path(), Some(first.path.as_path()));
    assert_eq!(controller.writer().writes.len(), 2);
}

#[test]
fn controller_treats_disappearing_old_cgroup_as_a_successful_revert() {
    let first = RecordingWriter::path("/user.slice/app.scope", 42);
    let resolver = StubResolver {
        results: [(42, Ok(first.clone()))].into_iter().collect(),
    };
    let mut controller = controller_fixture(resolver);
    let target = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(1).expect("window id"),
        pid: 42,
    };
    controller.reconcile(Some(target)).expect("apply");
    controller.writer_mut().missing = true;

    assert_eq!(
        controller
            .reconcile(None)
            .expect("disappearing cgroup is okay"),
        TransitionResult::Reverted
    );
    assert_eq!(controller.current_path(), None);
}

fn worker_fixture(name: &str, include_a_low: bool) -> (DmemPaths, PathBuf) {
    let root =
        std::env::temp_dir().join(format!("typhon-dmem-worker-{}-{name}", std::process::id()));
    let proc_root = root.join("proc");
    let cgroup_root = root.join("cgroup");
    std::fs::create_dir_all(&proc_root).expect("proc root");
    std::fs::create_dir_all(&cgroup_root).expect("cgroup root");
    std::fs::write(cgroup_root.join("cgroup.controllers"), "cpu dmem memory\n")
        .expect("controllers");
    std::fs::write(
        cgroup_root.join("dmem.capacity"),
        "region-a 10\nregion-b 20\n",
    )
    .expect("capacity");

    for (pid, cgroup) in [
        (1, "/typhon.scope"),
        (42, "/app-a.scope"),
        (43, "/app-b.scope"),
    ] {
        let proc_dir = proc_root.join(pid.to_string());
        let cgroup_dir = cgroup_root.join(cgroup.trim_start_matches('/'));
        std::fs::create_dir_all(&proc_dir).expect("process directory");
        std::fs::create_dir_all(&cgroup_dir).expect("cgroup directory");
        std::fs::write(
            proc_dir.join("status"),
            "Name:\tfixture\nUid:\t1000\t1000\t1000\t1000\n",
        )
        .expect("status");
        std::fs::write(
            proc_dir.join("stat"),
            "123 (fixture) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21",
        )
        .expect("stat");
        std::fs::write(proc_dir.join("cgroup"), format!("0::{cgroup}\n"))
            .expect("cgroup membership");
        if cgroup != "/app-a.scope" || include_a_low {
            std::fs::write(cgroup_dir.join("dmem.low"), "").expect("dmem.low");
        }
    }

    (
        DmemPaths {
            proc_root,
            cgroup_root,
        },
        root,
    )
}

fn wait_until<F>(mut condition: F)
where
    F: FnMut() -> bool,
{
    for _ in 0..500 {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("condition did not become true");
}

#[test]
fn worker_reconciles_latest_focus_and_reverts_on_shutdown() {
    let (paths, root) = worker_fixture("focus", true);
    let mut foreground = DmemForeground::start(DmemForegroundPolicy::Auto, paths, 1000, 1);
    let window_a = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(1).expect("window id"),
        pid: 42,
    };
    let window_b = ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(2).expect("window id"),
        pid: 43,
    };

    foreground.submit(Some(window_a));
    wait_until(|| foreground.snapshot().protected_cgroup.as_deref() == Some("app-a.scope"));
    foreground.submit(Some(window_b));
    wait_until(|| foreground.snapshot().protected_cgroup.as_deref() == Some("app-b.scope"));
    foreground.submit(None);
    wait_until(|| foreground.snapshot().protected_cgroup.is_none());
    assert!(foreground.snapshot().counters.transitions_applied >= 2);
    foreground.shutdown();
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn worker_accepts_pid_metadata_appearing_after_focus_and_skips_off_policy() {
    let (paths, root) = worker_fixture("late-pid", true);
    let mut foreground = DmemForeground::start(DmemForegroundPolicy::Auto, paths, 1000, 1);
    let window = crate::core::WindowId::from_raw(7).expect("window id");
    foreground.submit(None);
    foreground.submit(Some(ForegroundTarget {
        window_id: window,
        pid: 42,
    }));
    wait_until(|| foreground.snapshot().protected_cgroup.is_some());
    foreground.shutdown();
    std::fs::remove_dir_all(root).expect("cleanup");

    let (paths, root) = worker_fixture("off", true);
    let foreground = DmemForeground::start(DmemForegroundPolicy::Off, paths, 1000, 1);
    assert!(!foreground.snapshot().worker_available);
    drop(foreground);
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn retrying_target_is_superseded_by_newer_focus() {
    let (paths, root) = worker_fixture("retry", false);
    let mut foreground = DmemForeground::start(DmemForegroundPolicy::Auto, paths.clone(), 1000, 1);
    foreground.submit(Some(ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(1).expect("window id"),
        pid: 42,
    }));
    wait_until(|| foreground.snapshot().counters.retries > 0);
    foreground.submit(Some(ForegroundTarget {
        window_id: crate::core::WindowId::from_raw(2).expect("window id"),
        pid: 43,
    }));
    wait_until(|| foreground.snapshot().protected_cgroup.as_deref() == Some("app-b.scope"));
    assert!(foreground.snapshot().counters.stale_desired > 0);
    foreground.shutdown();
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn doctor_severity_follows_policy_and_runtime_health() {
    let base = DmemForegroundSnapshot {
        policy: DmemForegroundPolicy::Auto,
        kernel_capacity_available: false,
        worker_available: false,
        region_count: 0,
        desired_window_id: None,
        desired_pid: None,
        protected_cgroup: None,
        total_protected_capacity: None,
        degraded: false,
        last_failure: None,
        counters: DmemForegroundCounters::default(),
    };
    assert_eq!(
        base.doctor_severity(),
        crate::control_snapshots::DoctorSeverity::Ok
    );

    let mut forced = base.clone();
    forced.policy = DmemForegroundPolicy::On;
    forced.degraded = true;
    assert_eq!(
        forced.doctor_severity(),
        crate::control_snapshots::DoctorSeverity::Warning
    );

    let mut healthy = base;
    healthy.kernel_capacity_available = true;
    healthy.worker_available = true;
    healthy.protected_cgroup = Some("app.scope".to_owned());
    assert_eq!(
        healthy.doctor_severity(),
        crate::control_snapshots::DoctorSeverity::Ok
    );
}

#[test]
fn unsupported_capability_is_nonfatal_in_auto_and_degraded_in_on() {
    let (paths, root) = worker_fixture("unsupported", true);
    std::fs::remove_file(paths.cgroup_root.join("cgroup.controllers")).expect("remove controllers");
    let auto = DmemForeground::start(DmemForegroundPolicy::Auto, paths.clone(), 1000, 1);
    assert_eq!(
        auto.snapshot().doctor_severity(),
        crate::control_snapshots::DoctorSeverity::Ok
    );
    assert!(!auto.snapshot().worker_available);
    drop(auto);
    std::fs::remove_dir_all(root).expect("cleanup");

    let (paths, root) = worker_fixture("forced-unsupported", true);
    std::fs::remove_file(paths.cgroup_root.join("cgroup.controllers")).expect("remove controllers");
    let forced = DmemForeground::start(DmemForegroundPolicy::On, paths, 1000, 1);
    assert_eq!(
        forced.snapshot().doctor_severity(),
        crate::control_snapshots::DoctorSeverity::Warning
    );
    drop(forced);
    std::fs::remove_dir_all(root).expect("cleanup");
}
