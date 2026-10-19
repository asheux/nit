//! Compile the vendored AWK grammar. The parser and the external scanner are
//! both plain C (ABI 14), so unlike the Wolfram crate they build as a single
//! translation set into one archive. The `-Wno-*` flags silence what
//! tree-sitter's generated parser and the upstream scanner trip under cc's
//! default `-Wall -Wextra`: unused parameters, the scanner's unused
//! `tsawk_debug` helper and `had_whitespace` local, and an `int` vs `strlen`
//! comparison. None of them is a defect worth forking upstream over.

use std::path::Path;

fn main() {
    let src = Path::new("src");

    let mut build = cc::Build::new();
    build.include(src);
    build.flag_if_supported("-Wno-unused-parameter");
    build.flag_if_supported("-Wno-unused-but-set-variable");
    build.flag_if_supported("-Wno-unused-function");
    build.flag_if_supported("-Wno-unused-variable");
    build.flag_if_supported("-Wno-sign-compare");
    build.flag_if_supported("-Wno-trigraphs");
    build.file(src.join("parser.c"));
    build.file(src.join("scanner.c"));
    build.compile("tree-sitter-awk");

    println!("cargo:rerun-if-changed=src/parser.c");
    println!("cargo:rerun-if-changed=src/scanner.c");
    println!("cargo:rerun-if-changed=src/tree_sitter/parser.h");
}
