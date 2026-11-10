use serde_json::json;
use speleodb_api::types::{GitCommit, ProjectCommit};

#[test]
fn git_revision_timestamps_decode_as_unambiguous_instants() {
    let mut wire = json!({
        "author_email": "surveyor@example.invalid", "author_name": "Surveyor",
        "authored_date": "2026-11-10T03:18:23-05:00",
        "committed_date": "2026-11-10T09:19:45+01:00",
        "committer_email": "surveyor@example.invalid", "committer_name": "Surveyor",
        "dt_since": "2 minutes ago", "formats": [], "hexsha": "a".repeat(40),
        "hexsha_short": "aaaaaaa", "message": "Survey", "parents": [], "url": null
    });
    let commit: GitCommit = serde_json::from_value(wire.clone()).unwrap();
    let encoded = serde_json::to_value(commit).unwrap();
    assert_eq!(encoded["authored_date"], "2026-11-10T08:18:23Z");
    assert_eq!(encoded["committed_date"], "2026-11-10T08:19:45Z");
    for field in ["authored_date", "committed_date"] {
        let original = wire[field].clone();
        wire[field] = json!("2026/11/10 03:18");
        assert!(serde_json::from_value::<GitCommit>(wire.clone()).is_err());
        wire[field] = original;
    }
}

#[test]
fn cached_revision_timestamp_preserves_precision_and_rejects_ambiguous_dates() {
    let mut wire = json!({
        "author_email": "surveyor@example.invalid", "author_name": "Surveyor",
        "authored_date": "2026-11-10T09:18:23.123456+01:00",
        "dt_since": "2 minutes ago", "formats": [], "id": "a".repeat(40),
        "message": "Survey", "parent_ids": [], "tree": [], "url": null
    });
    let commit: ProjectCommit = serde_json::from_value(wire.clone()).unwrap();
    let encoded = serde_json::to_value(commit).unwrap();
    assert_eq!(encoded["authored_date"], "2026-11-10T08:18:23.123456Z");
    wire["authored_date"] = json!("2026/11/10 03:18");
    assert!(serde_json::from_value::<ProjectCommit>(wire).is_err());
}
