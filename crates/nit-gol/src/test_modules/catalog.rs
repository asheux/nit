use super::*;

fn parse_builtin_toml(source: &str) -> RuleCatalog {
    let builtin: RuleFile = toml::from_str(source).expect("parse builtin toml");
    let entries = builtin
        .rules
        .into_iter()
        .map(|raw| build_entry_from_file(raw, RuleSource::Builtin).expect("build entry"))
        .collect();
    RuleCatalog::from_entries(entries)
}

fn apply_overlay_toml(catalog: &mut RuleCatalog, source: &str, warnings: &mut Vec<String>) {
    let overlay: RuleOverlayFile = toml::from_str(source).expect("parse overlay toml");
    catalog.apply_overlays(&overlay.rules, warnings);
    catalog.rebuild_indices(warnings);
}

/// Every built-in rule has a unique (case-insensitive) id, a unique
/// canonical rulestring, and round-trips cleanly through the parser.
#[test]
fn builtins_unique_and_canonical() {
    let mut warnings = Vec::new();
    let catalog = RuleCatalog::load_builtin(&mut warnings);
    assert!(
        warnings.is_empty(),
        "built-in catalog loaded with warnings: {warnings:?}"
    );
    let mut seen_ids = HashSet::new();
    let mut seen_rules = HashSet::new();
    for entry in &catalog.entries {
        assert!(
            seen_ids.insert(entry.id.to_ascii_lowercase()),
            "duplicate built-in id: {}",
            entry.id
        );
        assert!(
            seen_rules.insert(rule_key(entry.rule)),
            "duplicate rulestring for {}: {}",
            entry.id,
            entry.rulestring
        );
        let parsed = Rule::parse(&entry.rulestring).expect("parse canonical");
        assert_eq!(
            parsed.to_string(),
            entry.rulestring,
            "{} rulestring must round-trip through the parser",
            entry.id
        );
    }
}

/// Overlays both modify existing entries (merge) and add entirely new
/// rules (create), with correct field propagation.
#[test]
fn overlay_merges_and_adds_rules() {
    let builtin_toml = r#"
[[rules]]
id = "base"
display_name = "Base"
rulestring = "B3/S23"
description = "Base rule"
tags = ["classic"]
aliases = ["base"]
"#;
    let overlay_toml = r#"
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
    let mut catalog = parse_builtin_toml(builtin_toml);
    let mut warnings = Vec::new();
    apply_overlay_toml(&mut catalog, overlay_toml, &mut warnings);
    assert!(
        warnings.is_empty(),
        "overlay application should not warn: {warnings:?}"
    );

    let base = catalog
        .find_by_id("base")
        .expect("base entry after overlay");
    assert_eq!(base.description, "Override rule", "merged description");
    assert_eq!(base.tags, vec!["override"], "merged tags");
    assert!(base.hidden, "overlay sets hidden = true");

    let custom = catalog
        .find_by_id("custom")
        .expect("custom entry added by overlay");
    assert_eq!(custom.rulestring, "B2/S", "custom rulestring");
    assert!(custom.favorite, "overlay sets favorite = true");
}
