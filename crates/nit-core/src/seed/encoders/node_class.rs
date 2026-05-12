//! Deterministic semantic mapping of an AST node kind to a 0-255 weight.
//! Declarations carry the most structural signal; literals and identifiers
//! the least. Used by `AstStructureEncoder` to project node identity onto
//! the genome.
//!
//! The table-driven prefix scan replaces a 6-arm `if kind.contains(...)`
//! cascade — same semantics, single linear loop, lower cyclomatic complexity.

struct ClassRow {
    weight: u8,
    needles: &'static [&'static str],
}

const CLASS_TABLE: &[ClassRow] = &[
    ClassRow {
        weight: 255,
        needles: &[
            "declaration",
            "definition",
            "function_item",
            "struct_item",
            "enum_item",
            "trait_item",
            "impl_item",
            "class_",
            "interface_",
        ],
    },
    ClassRow {
        weight: 210,
        needles: &[
            "if_", "match_", "switch_", "while_", "for_", "loop_", "try_", "catch_",
        ],
    },
    ClassRow {
        weight: 170,
        needles: &["expression", "call_", "binary_", "unary_", "assignment"],
    },
    ClassRow {
        weight: 130,
        needles: &["statement", "block"],
    },
    ClassRow {
        weight: 90,
        needles: &["type", "parameter", "argument"],
    },
    ClassRow {
        weight: 50,
        needles: &["literal", "string", "number"],
    },
];

const EXACT_DECLARATION: &str = "module";
const EXACT_STATEMENT: &[&str] = &["source_file", "program"];
const EXACT_LITERAL: &str = "identifier";

pub(super) fn ast_node_class(kind: &str) -> u8 {
    if kind == EXACT_DECLARATION {
        return 255;
    }
    if EXACT_STATEMENT.contains(&kind) {
        return 130;
    }
    if kind == EXACT_LITERAL {
        return 50;
    }
    for row in CLASS_TABLE {
        if row.needles.iter().any(|n| kind.contains(n)) {
            return row.weight;
        }
    }
    100
}
