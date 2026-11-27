//! Workspace-wide i18n audit test: fails on missing keys, duplicate keys, empty values,
//! or EN/VI parity drift across `en.ftl`, `vi.ftl`, and all `catalog.get("...")` calls.

use i18n_catalog::{Catalog, Locale};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn parse_ftl_keys(raw: &str) -> (BTreeSet<String>, Vec<String>, Vec<String>) {
    let mut keys = BTreeSet::new();
    let mut duplicates = Vec::new();
    let mut empties = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.trim().to_owned();
            let val = v.trim();
            if val.is_empty() {
                empties.push(key.clone());
            }
            if !keys.insert(key.clone()) {
                duplicates.push(key);
            }
        }
    }
    (keys, duplicates, empties)
}

fn collect_code_keys(dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect_code_keys(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && let Ok(text) = fs::read_to_string(&path)
        {
            let needle = "catalog.get(\"";
            let mut rest = text.as_str();
            while let Some(pos) = rest.find(needle) {
                let after = &rest[pos + needle.len()..];
                if let Some(end) = after.find('"') {
                    let key = &after[..end];
                    if !key.is_empty()
                        && key.bytes().all(|b| {
                            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
                        })
                    {
                        out.insert(key.to_owned());
                    }
                    rest = &after[end + 1..];
                } else {
                    break;
                }
            }
        }
    }
}

#[test]
fn en_and_vi_catalogs_have_strict_parity_and_cover_all_workspace_keys() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let en_raw = fs::read_to_string(manifest_dir.join("locales/en.ftl")).expect("read en.ftl");
    let vi_raw = fs::read_to_string(manifest_dir.join("locales/vi.ftl")).expect("read vi.ftl");

    let (en_keys, en_dups, en_empty) = parse_ftl_keys(&en_raw);
    let (vi_keys, vi_dups, vi_empty) = parse_ftl_keys(&vi_raw);

    assert!(en_dups.is_empty(), "duplicate keys in en.ftl: {en_dups:?}");
    assert!(vi_dups.is_empty(), "duplicate keys in vi.ftl: {vi_dups:?}");
    assert!(en_empty.is_empty(), "empty values in en.ftl: {en_empty:?}");
    assert!(vi_empty.is_empty(), "empty values in vi.ftl: {vi_empty:?}");

    let missing_in_vi: Vec<&String> = en_keys.difference(&vi_keys).collect();
    let extra_in_vi: Vec<&String> = vi_keys.difference(&en_keys).collect();
    assert!(
        missing_in_vi.is_empty(),
        "keys missing in vi.ftl: {missing_in_vi:?}"
    );
    assert!(
        extra_in_vi.is_empty(),
        "unused/extra keys in vi.ftl not in en.ftl: {extra_in_vi:?}"
    );

    let crates_dir = manifest_dir.parent().expect("crates dir");
    let mut code_keys = BTreeSet::new();
    collect_code_keys(crates_dir, &mut code_keys);
    assert!(
        !code_keys.is_empty(),
        "expected catalog.get calls in workspace"
    );

    let en_cat = Catalog::for_locale(Locale::En);
    let vi_cat = Catalog::for_locale(Locale::Vi);
    for key in &code_keys {
        assert!(
            en_cat.get(key).is_some(),
            "key `{key}` used in code is missing from en.ftl"
        );
        assert!(
            vi_cat.get(key).is_some(),
            "key `{key}` used in code is missing from vi.ftl"
        );
    }
}
