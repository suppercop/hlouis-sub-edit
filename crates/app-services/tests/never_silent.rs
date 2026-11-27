use app_services::{parse_github_releases_json, parse_github_releases_with_skipped, VersionError};

#[test]
fn skipped_releases_and_bad_shape_are_reported() {
    let (ok, skipped) = parse_github_releases_with_skipped(
        r#"[{"tag_name":"v1.2.3"},{"tag_name":"nightly"},{"name":"x"}]"#,
    )
    .unwrap();
    assert_eq!(ok.len(), 1);
    assert_eq!(skipped.len(), 2);
    assert!(skipped[0].contains("nightly"));
    assert!(matches!(
        parse_github_releases_json("42"),
        Err(VersionError::UnexpectedShape)
    ));
}
