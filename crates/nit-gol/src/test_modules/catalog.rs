use super::*;

const BUILTIN_TOML: &str = r#"
[[rules]]
id = "base"
display_name = "Base"
rulestring = "B3/S23"
description = "Base rule"
tags = ["classic"]
aliases = ["base"]
"#;

const OVERLAY_TOML: &str = r#"
[[rules]]
id = "base"
description = "Override rule"
tags = ["override"]
aliases = ["override"]
hidden = true

[[rules]]
id = "custom"
display_name = "Custom"
rulestring = "B2/S"
description = "Custom rule"
tags = ["custom"]
aliases = ["c"]
favorite = true
"#;

const DUPLICATE_RULESTRING_OVERLAY: &str = r#"
[[rules]]
id = "clone"
display_name = "Clone"
rulestring = "B3/S23"
description = "Same rulestring as 'base'"
"#;

fn catalog_from_toml(source: &str) -> RuleCatalog {
    let file: RuleFile = toml::from_str(source).expect("parse builtin toml");
    let entries = file
        .rules
        .into_iter()
        .map(|raw| build_entry_from_file(raw, RuleSource::Builtin).expect("build entry"))
        .collect();
    RuleCatalog::from_entries(entries)
}

fn merge_overlay_toml(catalog: &mut RuleCatalog, source: &str, warnings: &mut Vec<String>) {
    let overlay: RuleOverlayFile = toml::from_str(source).expect("parse overlay toml");
    catalog.apply_overlays(&overlay.rules, warnings);
    catalog.rebuild_indices(warnings);
}

/// Panics when `value` was already present; `field` and `rule_id`
/// make the failure pinpoint the duplicate entry.
fn insert_unique<T: std::hash::Hash + Eq + std::fmt::Debug>(
    seen: &mut HashSet<T>,
    value: T,
    field: &str,
    rule_id: &str,
) {
    assert!(
        seen.insert(value),
        "duplicate {field} for builtin id {rule_id}",
    );
}

/// Every built-in has a unique id and rulestring and round-trips cleanly through the parser.
#[test]
fn builtins_unique_and_canonical() {
    let mut warnings = Vec::new();
    let catalog = RuleCatalog::load_builtin(&mut warnings);
    assert!(
        warnings.is_empty(),
        "built-in catalog loaded with warnings: {warnings:?}",
    );

    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut seen_rule_keys: HashSet<u32> = HashSet::new();
    for entry in &catalog.entries {
        insert_unique(
            &mut seen_ids,
            entry.id.to_ascii_lowercase(),
            "id",
            &entry.id,
        );
        insert_unique(
            &mut seen_rule_keys,
            rule_key(entry.rule),
            "rulestring",
            &entry.id,
        );
        let parsed = Rule::parse(&entry.rulestring).expect("parse canonical");
        assert_eq!(
            parsed.to_string(),
            entry.rulestring,
            "{} rulestring must round-trip through the parser",
            entry.id,
        );
    }
}

/// Overlays merge into existing entries field-by-field and add new
/// entries when their id is unknown to the catalog.
#[test]
fn overlay_merges_and_adds_rules() {
    let mut catalog = catalog_from_toml(BUILTIN_TOML);
    let mut warnings = Vec::new();
    merge_overlay_toml(&mut catalog, OVERLAY_TOML, &mut warnings);
    assert!(
        warnings.is_empty(),
        "overlay application should not warn: {warnings:?}",
    );

    let merged = catalog
        .find_by_id("base")
        .expect("base entry after overlay");
    assert_eq!(merged.description, "Override rule", "merged description");
    assert_eq!(merged.tags, vec!["override"], "merged tags");
    assert!(merged.hidden, "overlay sets hidden = true");

    let added = catalog
        .find_by_id("custom")
        .expect("custom entry added by overlay");
    assert_eq!(added.rulestring, "B2/S", "custom rulestring");
    assert!(added.favorite, "overlay sets favorite = true");
}

/// A new-id overlay whose rulestring collides with an existing entry is
/// rejected with a warning; accepting it would produce two catalog
/// entries sharing the same `rule_key`, corrupting `find_by_rule`.
#[test]
fn overlay_rejects_duplicate_rulestring() {
    let mut catalog = catalog_from_toml(BUILTIN_TOML);
    let mut warnings = Vec::new();
    merge_overlay_toml(&mut catalog, DUPLICATE_RULESTRING_OVERLAY, &mut warnings);

    assert!(
        catalog.find_by_id("clone").is_none(),
        "duplicate-rulestring overlay must not be added as a new entry",
    );
    assert!(
        warnings.iter().any(|w| w.contains("duplicates rulestring")),
        "expected a duplicate-rulestring warning, got {warnings:?}",
    );
}
