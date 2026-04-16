use super::*;

/// Every built-in rule has a unique (case-insensitive) id, a unique
/// canonical rulestring, and round-trips cleanly through the parser.
#[test]
fn builtins_unique_and_canonical() {
    let mut warnings = Vec::new();
    let catalog = RuleCatalog::load_builtin(&mut warnings);
    assert!(warnings.is_empty());
    let mut seen_ids = HashSet::new();
    let mut seen_rules = HashSet::new();
    for entry in &catalog.entries {
        assert!(seen_ids.insert(entry.id.to_ascii_lowercase()));
        assert!(seen_rules.insert(rule_key(entry.rule)));
        let parsed = Rule::parse(&entry.rulestring).expect("parse canonical");
        assert_eq!(parsed.to_string(), entry.rulestring);
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
    let builtin: RuleFile = toml::from_str(builtin_toml).expect("builtin parse");
    let mut entries = Vec::new();
    for raw in builtin.rules {
        entries.push(build_entry_from_file(raw, RuleSource::Builtin).unwrap());
    }
    let mut catalog = RuleCatalog::from_entries(entries);

    let overlay: RuleOverlayFile = toml::from_str(overlay_toml).expect("overlay parse");
    let mut warnings = Vec::new();
    catalog.apply_overlays(&overlay.rules, &mut warnings);
    catalog.rebuild_indices(&mut warnings);
    assert!(warnings.is_empty());

    let base = catalog.find_by_id("base").expect("base present");
    assert_eq!(base.description, "Override rule");
    assert_eq!(base.tags, vec!["override"]);
    assert!(base.hidden);

    let custom = catalog.find_by_id("custom").expect("custom present");
    assert_eq!(custom.rulestring, "B2/S");
    assert!(custom.favorite);
}
