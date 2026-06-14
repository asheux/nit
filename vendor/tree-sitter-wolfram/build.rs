//! Compile the vendored Wolfram grammar. The parser is plain C (ABI 13); the
//! external scanner is C++ (uses `new`/`delete` and `<cwctype>`), so the two are
//! built as separate translation units. The parser is compiled first so its
//! unresolved `tree_sitter_wolfram_external_scanner_*` references are satisfied
//! by the scanner archive that follows it on the link line (GNU ld is
//! single-pass and order-sensitive; macOS ld64 is not).

use std::path::Path;

fn main() {
    let src = Path::new("src");

    let mut parser = cc::Build::new();
    parser.include(src);
    parser.flag_if_supported("-Wno-unused-parameter");
    parser.flag_if_supported("-Wno-unused-but-set-variable");
    parser.file(src.join("parser.c"));
    parser.compile("tree-sitter-wolfram-parser");

    let mut scanner = cc::Build::new();
    scanner.cpp(true);
    scanner.include(src);
    scanner.flag_if_supported("-Wno-unused-parameter");
    scanner.file(src.join("scanner.cc"));
    scanner.compile("tree-sitter-wolfram-scanner");

    println!("cargo:rerun-if-changed=src/parser.c");
    println!("cargo:rerun-if-changed=src/scanner.cc");
    println!("cargo:rerun-if-changed=src/tree_sitter/parser.h");
}
