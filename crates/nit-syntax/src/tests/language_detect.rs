use std::path::Path;

use crate::{LanguageId, LanguageRegistry};

#[test]
fn shebang_with_arguments_resolves_interpreter() {
    // Bug #1 regression: the pre-fix detect_shebang took the LAST
    // whitespace token, so flags or arg files (e.g. `-tt`, `-i a.py`)
    // were treated as the interpreter and the shebang silently fell
    // through to PlainText. Each input here reproduced the bug; all
    // must now resolve correctly.
    let cases: &[(&str, LanguageId)] = &[
        ("#!/usr/bin/python3 -tt", LanguageId::Python),
        ("#!/usr/bin/env python3 -i a.py", LanguageId::Python),
        ("#!/bin/bash -e", LanguageId::Bash),
        ("#!/usr/bin/env -S deno run", LanguageId::JavaScript),
    ];
    for &(line, expected) in cases {
        let detected = LanguageRegistry::detect(None, Some(line), None);
        assert_eq!(detected, expected, "shebang {line:?}");
    }
}

#[test]
fn shebang_basic_forms_still_work() {
    // Inputs that worked before Bug #1 must still work — guards
    // against an over-eager rewrite of the parser.
    let cases: &[(&str, LanguageId)] = &[
        ("#!/bin/sh", LanguageId::Bash),
        ("#!/usr/bin/python", LanguageId::Python),
        ("#!/usr/bin/env python3", LanguageId::Python),
        ("#!/usr/bin/env node", LanguageId::JavaScript),
    ];
    for &(line, expected) in cases {
        let detected = LanguageRegistry::detect(None, Some(line), None);
        assert_eq!(detected, expected, "shebang {line:?}");
    }
}

#[test]
fn shebang_unknown_interpreter_falls_through_to_plain() {
    let detected = LanguageRegistry::detect(None, Some("#!/usr/bin/perl"), None);
    assert_eq!(detected, LanguageId::PlainText);
}

#[test]
fn detect_explicit_override_beats_shebang() {
    let detected = LanguageRegistry::detect(
        Some(Path::new("script.py")),
        Some("#!/usr/bin/python3"),
        Some(LanguageId::Rust),
    );
    assert_eq!(detected, LanguageId::Rust);
}

#[test]
fn detect_shebang_beats_path_extension() {
    // For files saved with a misleading extension but a real shebang
    // (e.g. `script.txt` containing `#!/usr/bin/python3`), the
    // interpreter line wins over the path.
    let detected = LanguageRegistry::detect(
        Some(Path::new("script.txt")),
        Some("#!/usr/bin/python3"),
        None,
    );
    assert_eq!(detected, LanguageId::Python);
}

#[test]
fn detect_path_extension_lookup() {
    let cases: &[(&str, LanguageId)] = &[
        ("foo.rs", LanguageId::Rust),
        ("foo.py", LanguageId::Python),
        ("foo.tsx", LanguageId::TypeScript),
        ("foo.mjs", LanguageId::JavaScript),
        ("Cargo.toml", LanguageId::Toml),
        ("Makefile", LanguageId::Bash),
        ("readme.md", LanguageId::Markdown),
        ("style.scss", LanguageId::Css),
        ("config.yaml", LanguageId::Yaml),
    ];
    for &(name, expected) in cases {
        let detected = LanguageRegistry::detect(Some(Path::new(name)), None, None);
        assert_eq!(detected, expected, "path {name:?}");
    }
}

#[test]
fn detect_unknown_path_returns_plain() {
    let detected = LanguageRegistry::detect(Some(Path::new("data.bin")), None, None);
    assert_eq!(detected, LanguageId::PlainText);
}
