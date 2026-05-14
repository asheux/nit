//! Language detection: explicit override → shebang → path → `PlainText`.
//!
//! Every extension/filename/alias gate routes through
//! `nit_core::languages` so adding a language requires editing only the
//! `LANGUAGES` table (plus a grammar arm in `grammars.rs`).

use std::path::Path;

use nit_core::languages;

use super::id::LanguageId;

#[must_use]
pub(crate) fn detect(
    file_path: Option<&Path>,
    first_line: Option<&str>,
    explicit_override: Option<LanguageId>,
) -> LanguageId {
    if let Some(language) = explicit_override {
        return language;
    }
    if let Some(language) = first_line.and_then(detect_shebang) {
        return language;
    }
    file_path
        .and_then(detect_from_path)
        .unwrap_or(LanguageId::PlainText)
}

fn detect_from_path(file_path: &Path) -> Option<LanguageId> {
    languages::detect_by_path(file_path).and_then(|info| LanguageId::from_label(info.label))
}

fn detect_shebang(first_line: &str) -> Option<LanguageId> {
    // Bug-#1 regression guard: the pre-fix parser grabbed the LAST
    // whitespace token, so flags or arg files (e.g. `-tt`, `-i a.py`,
    // `env -S deno run`) were treated as the interpreter and the
    // shebang silently fell through. This positional walker keeps the
    // first non-flag token after `env` and ignores anything after it.
    let after_hash = first_line.trim().strip_prefix("#!")?;
    let mut tokens = after_hash.split_whitespace();
    let basename = |tok: &str| tok.rsplit('/').next().unwrap_or("").to_lowercase();

    let first = tokens.next()?;
    let mut name = basename(first);
    if name == "env" {
        let interpreter = tokens.find(|tok| !tok.starts_with('-'))?;
        name = basename(interpreter);
    }
    languages::detect_by_shebang(&name).and_then(|info| LanguageId::from_label(info.label))
}

/// Resolves Markdown/HTML fenced-block info-string tags like `rust,no_run`,
/// `tsx`, or `shell-session`. The token is split on the first
/// non-alphanumeric/`-`/`_` character so info-string extras (`rust,no_run`,
/// `python title="x"`) reduce to their language label before lookup.
#[must_use]
pub(crate) fn from_injection_name(injection_name: &str) -> Option<LanguageId> {
    let token = injection_name
        .split(|ch: char| !ch.is_alphanumeric() && ch != '-' && ch != '_')
        .next()
        .unwrap_or(injection_name);
    languages::detect_by_injection_alias(token).and_then(|info| LanguageId::from_label(info.label))
}
