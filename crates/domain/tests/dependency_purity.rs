//! The dependency contract of the `domain` crate, enforced as a test.
//!
//! `domain` is allowed exactly one runtime dependency: `serde`. This test
//! reads the crate's own manifest and fails the build if that stops being
//! true, so the rule cannot rot into a comment that nobody checks.
//!
//! The manifest is parsed with the standard library rather than a TOML
//! crate. That is deliberate: a test which proves the manifest contains only
//! `serde` should not need its own dependency to do it, and `[dependencies]`
//! is a flat table of `name = ...` lines that twenty lines of scanning read
//! correctly.

use std::collections::BTreeSet;
use std::path::Path;

/// The only crate `domain` is permitted to depend on.
const ALLOWED: &[&str] = &["serde"];

/// Crates that must never appear in `domain`'s dependency list. These are the
/// ones that would make the domain testable only by running infrastructure,
/// and they are also the ones `domain` has no business knowing about.
const FORBIDDEN: &[&str] = &[
    "adapters",
    "application",
    "sqlx",
    "redis",
    "axum",
    "tower",
    "tower-http",
    "reqwest",
    "hyper",
    "actix",
    "tokio",
    "serde_json",
    "thiserror",
    "anyhow",
    "uuid",
    "chrono",
];

/// The manifest tables whose keys are dependency names. `dev-dependencies` is
/// excluded on purpose: a test-only dependency is not part of anything that
/// imports `domain`.
const DEPENDENCY_TABLES: &[&str] = &["dependencies"];

/// Table-header markers that would hide a dependency from this test.
const UNINSPECTED_TABLE_MARKERS: &[&str] = &["dependencies.", "target.", "patch.", "replace."];

fn read_manifest() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    std::fs::read_to_string(&path).expect("domain manifest is readable")
}

/// Every top-level table header in the manifest, e.g. `dependencies`.
fn table_headers(manifest: &str) -> Vec<String> {
    manifest
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix('[')?.strip_suffix(']'))
        .map(ToString::to_string)
        .collect()
}

/// The dependency names declared in the given tables of a manifest.
///
/// A line counts as a dependency when it is a bare `name = ...` assignment
/// inside one of `tables`. Dotted keys are skipped: they are not dependency
/// names, and the manifest in this repository uses none of them inside
/// `[dependencies]`.
fn declared_dependencies(manifest: &str, tables: &[&str]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut in_table = false;

    for raw in manifest.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let name = rest.strip_suffix(']').unwrap_or(rest);
            in_table = !name.contains('.') && tables.contains(&name);
            continue;
        }
        if !in_table {
            continue;
        }
        if let Some((key, _value)) = line.split_once('=') {
            let key = key.trim();
            let is_bare_key = !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if is_bare_key {
                found.insert(key.to_string());
            }
        }
    }

    found
}

#[test]
fn manifest_parsing_finds_the_dependencies_it_is_meant_to_find() {
    // A purity test that silently reads zero dependencies passes forever. This
    // asserts the parser still works before anything relies on its result.
    let manifest = read_manifest();
    assert_eq!(
        declared_dependencies(&manifest, DEPENDENCY_TABLES),
        BTreeSet::from(["serde".to_string()]),
        "dependency_purity.rs failed to read [dependencies]; the test below \
         would be vacuous"
    );
}

#[test]
fn domain_declares_only_serde_as_a_runtime_dependency() {
    let deps = declared_dependencies(&read_manifest(), DEPENDENCY_TABLES);
    let allowed: BTreeSet<String> = ALLOWED.iter().map(|s| (*s).to_string()).collect();
    let offenders: Vec<&String> = deps.difference(&allowed).collect();

    assert!(
        offenders.is_empty(),
        "domain may depend on {ALLOWED:?} only, but also depends on {offenders:?}. \
         Each extra dependency is another place the money path can acquire a \
         representation or a rounding rule that the domain did not choose."
    );
}

#[test]
fn domain_manifest_names_no_infrastructure_crate() {
    let deps = declared_dependencies(&read_manifest(), DEPENDENCY_TABLES);

    for forbidden in FORBIDDEN {
        assert!(
            !deps.contains(*forbidden),
            "domain declares a dependency on {forbidden}, which breaks the \
             infrastructure-free contract"
        );
    }
}

#[test]
fn every_dependency_table_in_the_manifest_is_one_this_test_inspects() {
    // A dependency the test cannot see is a dependency it cannot forbid, so a
    // table header it does not understand is a build failure rather than a
    // gap in coverage nobody notices.
    for header in table_headers(&read_manifest()) {
        for marker in UNINSPECTED_TABLE_MARKERS {
            assert!(
                !header.contains(marker),
                "manifest contains the table header `[{header}]`, which \
                 dependency_purity.rs does not inspect. Extend the test or \
                 remove the table."
            );
        }
    }
}
