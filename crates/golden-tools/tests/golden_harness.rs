//! Integration test comparing all 10 `golden-tools` fixtures against expected `.ass` files.

use golden_tools::{run_golden_case, GOLDEN_CASES};
use std::fs;
use std::path::Path;

#[test]
fn all_10_golden_ass_fixtures_match_expected() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let update = std::env::var("UPDATE_GOLDEN").ok().as_deref() == Some("1");

    for case in GOLDEN_CASES {
        let in_path = base.join("input").join(format!("{case}.ass"));
        let exp_path = base.join("expected").join(format!("{case}.ass"));

        let input = fs::read_to_string(&in_path)
            .unwrap_or_else(|e| panic!("missing input fixture {}: {e}", in_path.display()));
        let actual = run_golden_case(case, &input)
            .unwrap_or_else(|e| panic!("golden case {case} failed: {e}"));

        if update {
            fs::create_dir_all(base.join("expected")).expect("mkdir expected");
            fs::write(&exp_path, &actual).expect("write expected");
            continue;
        }

        let expected = fs::read_to_string(&exp_path)
            .unwrap_or_else(|e| panic!("missing expected fixture {}: {e}", exp_path.display()));
        assert_eq!(
            actual.replace("\r\n", "\n"),
            expected.replace("\r\n", "\n"),
            "golden mismatch for case `{case}`"
        );
    }
}
