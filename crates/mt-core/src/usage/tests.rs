use super::*;
use crate::review::{ByteRange, ReviewSource, SourceSnapshot};

// Every fixture uses the same production named mutex, whose bounded wait is
// deliberately short. Do not turn test-thread queueing into storage failures.
pub(crate) static USAGE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn at_day(day: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(day * SECONDS_PER_DAY)
}

fn enabled_store(root: &Path) -> (UsageStore, std::sync::MutexGuard<'static, ()>) {
    let guard = USAGE_TEST_LOCK.lock().unwrap();
    std::fs::create_dir_all(root).unwrap();
    let consent = root.join("settings.toml");
    std::fs::write(&consent, "usage-metrics-enabled = true\n").unwrap();
    (UsageStore::open_in(root, &consent, true), guard)
}

fn events() -> [UsageEvent; 16] {
    [
        UsageEvent::FirstLaunch,
        UsageEvent::ArtifactOpened,
        UsageEvent::ArtifactPasted,
        UsageEvent::ReviewStarted {
            scope: UsageReviewScope::Document,
            lens: ArtifactLens::Plan,
        },
        UsageEvent::ReviewCompleted(ReviewDurationBucket::UnderOneMinute),
        UsageEvent::ClarificationAnswered,
        UsageEvent::ClarificationDeferred,
        UsageEvent::ClarificationRejected,
        UsageEvent::RevisionDecision {
            accepted_hunks: 2,
            rejected_hunks: 3,
        },
        UsageEvent::RevisionApplied,
        UsageEvent::ArtifactSaved,
        UsageEvent::ArtifactCopied,
        UsageEvent::ProviderFailure,
        UsageEvent::CredentialFailure,
        UsageEvent::StaleResultBlocked,
        UsageEvent::RecoveryRestored,
    ]
}

#[test]
fn day_bucketing_discards_time_of_day_and_uses_utc() {
    assert_eq!(day_bucket(UNIX_EPOCH - Duration::from_secs(1)), 0);
    assert_eq!(day_bucket(UNIX_EPOCH), 0);
    assert_eq!(day_bucket(at_day(42) + Duration::from_secs(86_399)), 42);
    assert_eq!(day_bucket(at_day(43)), 43);
}

#[test]
fn duration_buckets_have_fixed_coarse_boundaries() {
    for (seconds, expected) in [
        (0, ReviewDurationBucket::UnderOneMinute),
        (59, ReviewDurationBucket::UnderOneMinute),
        (60, ReviewDurationBucket::OneToFiveMinutes),
        (299, ReviewDurationBucket::OneToFiveMinutes),
        (300, ReviewDurationBucket::FiveToFifteenMinutes),
        (899, ReviewDurationBucket::FiveToFifteenMinutes),
        (900, ReviewDurationBucket::FifteenMinutesOrMore),
        (u64::MAX, ReviewDurationBucket::FifteenMinutesOrMore),
    ] {
        assert_eq!(
            ReviewDurationBucket::from_duration(Duration::from_secs(seconds)),
            expected
        );
    }
}

#[test]
fn disabled_recording_creates_no_files_or_directories() {
    let _guard = USAGE_TEST_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("not-created");
    let store = UsageStore::open_in(&root, &directory.path().join("settings.toml"), false);
    assert!(!store.enabled());
    for event in events() {
        store.record(event, at_day(200)).unwrap();
    }
    assert!(store.inspect(at_day(200)).unwrap().days().is_empty());
    assert!(!root.exists());
}

#[test]
fn daily_schema_aggregates_events_lenses_durations_and_hunk_counts() {
    let directory = tempfile::tempdir().unwrap();
    let (store, _guard) = enabled_store(directory.path());
    for event in events() {
        store.record(event, at_day(200)).unwrap();
    }
    store
        .record(
            UsageEvent::ArtifactOpened,
            at_day(200) + Duration::from_secs(123),
        )
        .unwrap();
    store
        .record(
            UsageEvent::ReviewStarted {
                scope: UsageReviewScope::Selection,
                lens: ArtifactLens::AgentInstructions,
            },
            at_day(200),
        )
        .unwrap();
    store
        .record(UsageEvent::ArtifactOpened, at_day(201))
        .unwrap();

    let bytes = std::fs::read(directory.path().join("usage.json")).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json.as_object().unwrap().len(), 2);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["days"].as_object().unwrap().len(), 2);
    assert_eq!(json["days"]["200"]["events"].as_object().unwrap().len(), 16);
    assert_eq!(json["days"]["200"]["events"]["artifact_opened"], 2);
    assert_eq!(json["days"]["200"]["review_started"]["document"]["plan"], 1);
    assert_eq!(
        json["days"]["200"]["review_started"]["selection"]["agent_instructions"],
        1
    );
    assert_eq!(
        json["days"]["200"]["review_durations"]["under_one_minute"],
        1
    );
    assert_eq!(json["days"]["200"]["accepted_hunks"], 2);
    assert_eq!(json["days"]["200"]["rejected_hunks"], 3);
    assert_eq!(
        store.inspect(at_day(201)).unwrap(),
        serde_json::from_slice::<UsageAggregate>(&bytes).unwrap()
    );
}

#[test]
fn pruning_keeps_exactly_180_day_old_entries_and_removes_older_and_future_days_on_load() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("usage.json");
    let mut aggregate = UsageAggregate::default();
    for day in [19, 20, 200, 201] {
        aggregate
            .days
            .entry(day)
            .or_default()
            .record(UsageEvent::ArtifactOpened);
    }
    std::fs::write(&path, serde_json::to_vec(&aggregate).unwrap()).unwrap();
    let (store, _guard) = enabled_store(directory.path());
    let counts = store.inspect(at_day(200)).unwrap();
    assert_eq!(counts.days().keys().copied().collect::<Vec<_>>(), [20, 200]);
    let persisted: UsageAggregate = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(persisted, counts);
}

#[test]
fn recording_prunes_before_save_and_bounds_retention() {
    let directory = tempfile::tempdir().unwrap();
    let (store, _guard) = enabled_store(directory.path());
    store
        .record(UsageEvent::ArtifactOpened, at_day(20))
        .unwrap();
    store
        .record(UsageEvent::ArtifactOpened, at_day(21))
        .unwrap();
    store
        .record(UsageEvent::ArtifactOpened, at_day(201))
        .unwrap();
    let persisted: UsageAggregate =
        serde_json::from_slice(&std::fs::read(directory.path().join("usage.json")).unwrap())
            .unwrap();
    assert_eq!(
        persisted.days().keys().copied().collect::<Vec<_>>(),
        [21, 201]
    );
}

#[test]
fn disabling_preserves_store_bytes_and_inspection_and_export_still_prune_in_memory() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("usage.json");
    let (mut store, _guard) = enabled_store(directory.path());
    store.record(UsageEvent::FirstLaunch, at_day(1)).unwrap();
    let original = std::fs::read(&path).unwrap();
    store.set_enabled(false);
    store
        .record(UsageEvent::ArtifactPasted, at_day(200))
        .unwrap();
    assert!(store.inspect(at_day(200)).unwrap().days().is_empty());
    let exported: serde_json::Value =
        serde_json::from_slice(&store.export_json(at_day(200)).unwrap()).unwrap();
    assert!(exported["days"].as_object().unwrap().is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    store.set_enabled(true);
    store
        .record(UsageEvent::ArtifactOpened, at_day(200))
        .unwrap();
    assert_eq!(store.inspect(at_day(200)).unwrap().days().len(), 1);
}

#[test]
fn export_contains_only_version_and_aggregates_and_never_overwrites_a_file() {
    let directory = tempfile::tempdir().unwrap();
    let (store, _guard) = enabled_store(&directory.path().join("data"));
    store
        .record(UsageEvent::ArtifactCopied, at_day(200))
        .unwrap();
    let export = directory.path().join("chosen-summary.json");
    assert!(!export.exists());
    store.export_to(&export, at_day(200)).unwrap();
    let bytes = std::fs::read(&export).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        json.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["app_version", "days", "schema_version"]
    );
    assert_eq!(json["app_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["days"]["200"]["events"]["artifact_copied"], 1);
    assert_eq!(
        store.export_to(&export, at_day(200)),
        Err(UsageError::Io(io::ErrorKind::AlreadyExists))
    );
    assert_eq!(std::fs::read(&export).unwrap(), bytes);
}

#[test]
fn clear_deletes_the_store_without_resurrecting_old_counts_or_deleting_exports() {
    let directory = tempfile::tempdir().unwrap();
    let (mut store, _guard) = enabled_store(directory.path());
    store
        .record(UsageEvent::ArtifactSaved, at_day(200))
        .unwrap();
    let export = directory.path().join("export.json");
    store.export_to(&export, at_day(200)).unwrap();
    store.set_enabled(false);
    store.clear().unwrap();
    store.clear().unwrap();
    assert!(!directory.path().join("usage.json").exists());
    assert!(export.exists());
    assert!(store.inspect(at_day(200)).unwrap().days().is_empty());
    store.set_enabled(true);
    store
        .record(UsageEvent::RecoveryRestored, at_day(200))
        .unwrap();
    let counts = store.inspect(at_day(200)).unwrap();
    assert_eq!(counts.days()[&200].events.len(), 1);
    assert_eq!(
        counts.days()[&200].events[&UsageEventKind::RecoveryRestored],
        1
    );
}

#[test]
fn recording_reduces_sentinel_content_path_and_fake_key_before_storage_or_export() {
    let document = "PRIVATE-DOCUMENT-SENTINEL";
    let path = "C:/PRIVATE-PATH-SENTINEL/AGENTS.md";
    let key = "FAKE-API-KEY-SENTINEL";
    let request = ReviewRequest::new(
        ArtifactLens::infer_from_path(path),
        ReviewScope::Document,
        ReviewSource::document_at(format!("{document}\n{key}"), path),
        SourceSnapshot::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (store, _guard) = enabled_store(directory.path());
    let event = UsageEvent::review_started(&request);
    store
        .record(event, at_day(200) + Duration::from_secs(43_210))
        .unwrap();
    let exported_path = directory.path().join("summary.json");
    store.export_to(&exported_path, at_day(200)).unwrap();
    for bytes in [
        std::fs::read(directory.path().join("usage.json")).unwrap(),
        store.export_json(at_day(200)).unwrap(),
        std::fs::read(exported_path).unwrap(),
        format!("{event:?}").into_bytes(),
    ] {
        let text = String::from_utf8(bytes).unwrap();
        for sentinel in [
            document,
            path,
            key,
            "AGENTS.md",
            "PRIVATE-PATH-SENTINEL",
            "43210",
        ] {
            assert!(
                !text.contains(sentinel),
                "prohibited content reached usage output"
            );
        }
    }
    assert_eq!(
        store.inspect(at_day(200)).unwrap().days()[&200].review_started
            [&UsageReviewScope::Document][&ArtifactLens::AgentInstructions],
        1
    );
}

#[test]
fn selection_reduction_keeps_only_the_scope_category_not_selection_offsets() {
    let request = ReviewRequest::selection(
        ArtifactLens::Prompt,
        "private selection source",
        ByteRange::new(8, 17).unwrap(),
        SourceSnapshot::new(19, 23),
    )
    .unwrap();
    assert_eq!(
        UsageEvent::review_started(&request),
        UsageEvent::ReviewStarted {
            scope: UsageReviewScope::Selection,
            lens: ArtifactLens::Prompt,
        }
    );
}

#[test]
fn malformed_unknown_or_content_bearing_store_is_not_rewritten_or_exported() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("usage.json");
    let (store, _guard) = enabled_store(directory.path());
    for text in [
        r#"{"schema_version":1,"days":{},"content":"PRIVATE-INPUT"}"#,
        r#"{"schema_version":1,"days":{"PRIVATE-INPUT":{}}}"#,
        r#"{"schema_version":1,"days":{"200":{"events":{"PRIVATE-INPUT":1},"review_started":{},"review_durations":{},"accepted_hunks":0,"rejected_hunks":0}}}"#,
        "PRIVATE-INPUT",
    ] {
        std::fs::write(&path, text).unwrap();
        let error = store
            .record(UsageEvent::ArtifactOpened, at_day(200))
            .unwrap_err();
        assert_eq!(error, UsageError::InvalidStore);
        assert!(!format!("{error:?} {error}").contains("PRIVATE-INPUT"));
        assert_eq!(
            store.export_json(at_day(200)),
            Err(UsageError::InvalidStore)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
    std::fs::write(&path, r#"{"schema_version":2,"days":{}}"#).unwrap();
    assert_eq!(
        store.inspect(at_day(200)),
        Err(UsageError::UnsupportedSchema)
    );
}

#[test]
fn aggregate_counts_saturate_instead_of_overflowing() {
    let mut counts = UsageDayCounts {
        accepted_hunks: u64::MAX,
        rejected_hunks: u64::MAX,
        ..UsageDayCounts::default()
    };
    counts
        .events
        .insert(UsageEventKind::RevisionDecision, u64::MAX);
    counts.record(UsageEvent::RevisionDecision {
        accepted_hunks: 1,
        rejected_hunks: 1,
    });
    assert_eq!(counts.accepted_hunks, u64::MAX);
    assert_eq!(counts.rejected_hunks, u64::MAX);
    assert_eq!(counts.events[&UsageEventKind::RevisionDecision], u64::MAX);
}

#[test]
fn revoked_persisted_consent_stops_an_already_enabled_handle_and_retention_writes() {
    let directory = tempfile::tempdir().unwrap();
    let (store, _guard) = enabled_store(directory.path());
    store.record(UsageEvent::ArtifactOpened, at_day(1)).unwrap();
    let path = directory.path().join("usage.json");
    let original = std::fs::read(&path).unwrap();
    std::fs::write(
        directory.path().join("settings.toml"),
        "usage-metrics-enabled = false\n",
    )
    .unwrap();
    assert!(store.enabled());
    store
        .record(UsageEvent::ArtifactSaved, at_day(200))
        .unwrap();
    assert!(store.inspect(at_day(200)).unwrap().days().is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn absent_unreadable_malformed_or_missing_consent_fails_closed_without_creating_data() {
    let _guard = USAGE_TEST_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let consent = directory.path().join("settings.toml");
    let store = UsageStore::open_in(&root, &consent, true);
    store
        .record(UsageEvent::ArtifactOpened, at_day(200))
        .unwrap();
    assert!(!root.exists());
    std::fs::create_dir(&consent).unwrap();
    store
        .record(UsageEvent::ArtifactOpened, at_day(200))
        .unwrap();
    assert!(!root.exists());
    std::fs::remove_dir(&consent).unwrap();
    for settings in [
        "usage-metrics-enabled = [",
        "theme = 'dark'",
        "usage-metrics-enabled = false",
    ] {
        std::fs::write(&consent, settings).unwrap();
        store
            .record(UsageEvent::ArtifactOpened, at_day(200))
            .unwrap();
        assert!(!root.exists());
    }
    std::fs::write(&consent, "usage-metrics-enabled = true").unwrap();
    store
        .record(UsageEvent::ArtifactOpened, at_day(200))
        .unwrap();
    assert_eq!(
        store.inspect(at_day(200)).unwrap().days()[&200].events[&UsageEventKind::ArtifactOpened],
        1
    );
}

#[test]
fn save_and_clear_remove_only_usage_owned_staging_files() {
    let directory = tempfile::tempdir().unwrap();
    let (store, _guard) = enabled_store(directory.path());
    let stale = directory.path().join(".markturbo-usage-AbCd12.tmp");
    let unrelated = directory.path().join("unrelated.tmp");
    let wrong_suffix = directory.path().join(".markturbo-usage-keep.txt");
    let named_directory = directory.path().join(".markturbo-usage-abcdef.tmp");
    std::fs::create_dir(&named_directory).unwrap();
    for path in [&stale, &unrelated, &wrong_suffix] {
        std::fs::write(path, "not content to collect").unwrap();
    }
    store
        .record(UsageEvent::ArtifactSaved, at_day(200))
        .unwrap();
    assert!(!stale.exists());
    assert!(directory.path().join("usage.json").exists());
    assert!(unrelated.exists());
    assert!(wrong_suffix.exists());
    assert!(named_directory.is_dir());
    std::fs::write(&stale, "stale stage").unwrap();
    store.clear().unwrap();
    assert!(!stale.exists());
    assert!(!directory.path().join("usage.json").exists());
    assert!(directory.path().join("settings.toml").exists());
    assert!(unrelated.exists());
    assert!(wrong_suffix.exists());
    assert!(named_directory.is_dir());
}
