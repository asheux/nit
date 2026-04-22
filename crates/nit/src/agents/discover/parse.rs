pub(super) fn parse_model_list_from_output(raw_stdout: &[u8]) -> Vec<String> {
    let decoded_output = String::from_utf8_lossy(raw_stdout);
    let trimmed_content = decoded_output.trim();
    if trimmed_content.is_empty() {
        return Vec::new();
    }

    if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(trimmed_content) {
        let mut collected = Vec::new();
        extract_models_from_json(&json_value, &mut collected);
        collected.sort();
        collected.dedup();
        return collected;
    }

    parse_text_model_list(trimmed_content)
}

fn parse_text_model_list(content: &str) -> Vec<String> {
    let mut model_ids = Vec::new();
    for output_line in content.lines() {
        let cleaned = output_line
            .trim()
            .trim_start_matches(['-', '*', '•'])
            .trim();
        if cleaned.is_empty() {
            continue;
        }
        let Some(first_token) = cleaned.split_whitespace().next() else {
            continue;
        };
        if first_token.ends_with(':') || first_token.len() < 3 {
            continue;
        }
        if first_token.eq_ignore_ascii_case("models") || first_token.eq_ignore_ascii_case("model") {
            continue;
        }
        model_ids.push(first_token.to_string());
    }
    model_ids.sort();
    model_ids.dedup();
    model_ids
}

fn extract_models_from_json(json_node: &serde_json::Value, collector: &mut Vec<String>) {
    match json_node {
        serde_json::Value::String(text) => {
            let cleaned = text.trim();
            if !cleaned.is_empty() {
                collector.push(cleaned.to_string());
            }
        }
        serde_json::Value::Array(elements) => {
            for child in elements {
                extract_models_from_json(child, collector);
            }
        }
        serde_json::Value::Object(fields) => {
            if let Some(identity_value) = first_identity_field(fields) {
                collector.push(identity_value);
                return;
            }
            for container_key in ["models", "data"] {
                if let Some(nested_value) = fields.get(container_key) {
                    extract_models_from_json(nested_value, collector);
                }
            }
        }
        _ => {}
    }
}

fn first_identity_field(fields: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    ["id", "name", "model", "slug"]
        .iter()
        .find_map(|key| match fields.get(*key) {
            Some(serde_json::Value::String(text)) => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            _ => None,
        })
}
