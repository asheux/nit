//! `gd` goto-definition resolver: the heuristic, language-agnostic same-file
//! scan behind `Action::GotoDefinition`. Covers keyword-prefixed definitions,
//! leading assignments, whole-word boundaries, and non-matches.

use crate::state::find_definition_line;

#[test]
fn finds_rust_fn_definition_skipping_the_call_site() {
    let lines = ["fn main() {", "    helper();", "}", "fn helper() {}"];
    assert_eq!(find_definition_line(&lines, "helper"), Some(3));
}

#[test]
fn finds_struct_and_class_definitions() {
    let rust = ["use x;", "pub struct Widget {", "}"];
    assert_eq!(find_definition_line(&rust, "Widget"), Some(1));
    let python = ["import os", "class Parser:", "    pass"];
    assert_eq!(find_definition_line(&python, "Parser"), Some(1));
}

#[test]
fn finds_python_def_over_an_earlier_call() {
    let lines = ["greet(name)", "def greet(name):", "    return name"];
    assert_eq!(find_definition_line(&lines, "greet"), Some(1));
}

#[test]
fn finds_a_leading_assignment() {
    let lines = ["    args = build_parser().parse_args()"];
    assert_eq!(find_definition_line(&lines, "args"), Some(0));
}

#[test]
fn respects_whole_word_boundaries() {
    // `argsmap = ...` must not satisfy a `gd` on `args`.
    let lines = ["argsmap = 1", "args = 2"];
    assert_eq!(find_definition_line(&lines, "args"), Some(1));
}

#[test]
fn ignores_usages_and_comparisons() {
    let lines = ["let y = foo + bar;", "if foo == bar {"];
    assert_eq!(find_definition_line(&lines, "foo"), None);
    assert_eq!(find_definition_line(&lines, "bar"), None);
}

#[test]
fn resolves_a_unicode_identifier() {
    let lines = ["fn café() {}", "    café();"];
    assert_eq!(find_definition_line(&lines, "café"), Some(0));
}
