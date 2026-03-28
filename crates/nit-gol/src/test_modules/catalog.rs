use super::*;

#[test]
fn builtins_unique_and_canonical() {
    let mut warnings = Vec::new();
    let catalog = RuleCatalog::load_builtin(&mut warnings);
    assert!(warnings.is_empty());
    let mut ids = HashSet::new();
    let mut rules = HashSet::new();
    for entry in catalog.entries.iter() {
        assert!(ids.insert(entry.id.to_ascii_lowercase()));
        assert!(rules.insert(rule_key(entry.rule)));
        let parsed = Rule::parse(&entry.rulestring).expect("parse canonical");
        assert_eq!(parsed.to_string(), entry.rulestring);
    }
}

#[test]
fn overlay_merges_and_adds_rules() {
    let builtin = r#"
[[rules]]
id = "base"
display_name = "Base"
rulestring = "B3/S23"
description = "Base rule"
tags = ["classic"]
aliases = ["base"]
"#;
    let overlay = r#"
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
    let base_file: RuleFile = toml::from_str(builtin).expect("builtin parse");
    let mut entries = Vec::new();
    for entry in base_file.rules {
        entries.push(build_entry_from_file(entry, RuleSource::Builtin).unwrap());
    }
    let mut catalog = RuleCatalog::from_entries(entries);
    let overlay_file: RuleOverlayFile = toml::from_str(overlay).expect("overlay parse");
    let overlays = overlay_file.rules;
    let mut warnings = Vec::new();
    catalog.apply_overlays(&overlays, &mut warnings);
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
