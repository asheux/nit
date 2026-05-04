use std::collections::HashMap;

pub(crate) fn parse_gemini_models_from_source(js_source: &str) -> Vec<String> {
    let const_bindings: HashMap<String, String> =
        js_source.lines().filter_map(parse_js_const_line).collect();
    let set_member_tokens = extract_valid_models_set_members(js_source);

    let mut resolved: Vec<String> = set_member_tokens
        .into_iter()
        .filter_map(|token| {
            strip_single_quotes(token)
                .map(|v| v.to_string())
                .or_else(|| const_bindings.get(token).cloned())
        })
        .collect();
    resolved.sort();
    resolved.dedup();
    resolved
}

fn parse_js_const_line(line: &str) -> Option<(String, String)> {
    let after_export = line.trim().strip_prefix("export const ")?;
    let (binding_name, rhs) = after_export.split_once('=')?;
    let cleaned_rhs = rhs.trim().trim_end_matches(';').trim();
    let unquoted = strip_single_quotes(cleaned_rhs)?;
    Some((binding_name.trim().to_string(), unquoted.to_string()))
}

fn extract_valid_models_set_members(js_source: &str) -> Vec<&str> {
    let set_constructor_prefix = "export const VALID_GEMINI_MODELS = new Set([";

    let Some(prefix_offset) = js_source.find(set_constructor_prefix) else {
        return Vec::new();
    };

    let inner_content = &js_source[prefix_offset + set_constructor_prefix.len()..];

    let Some(terminator_offset) = inner_content.find("]);") else {
        return Vec::new();
    };

    inner_content[..terminator_offset]
        .split(',')
        .map(|element| element.trim())
        .filter(|element| !element.is_empty())
        .collect()
}

fn strip_single_quotes(text: &str) -> Option<&str> {
    let inner = text.trim().strip_prefix('\'')?.strip_suffix('\'')?;
    (!inner.is_empty()).then_some(inner)
}
