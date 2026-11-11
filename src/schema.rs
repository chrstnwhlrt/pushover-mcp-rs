//! Input schemas in the shape small models read best.
//!
//! Measured with Gemma 4 E4B in otomo (November 2026): a schema that points
//! to `$defs` with `$ref`, or lists enum values as `oneOf` constants, loses
//! parts on the way to the model, which then sent `"priority": 1`. The
//! transformation here is the one that was measured: every local `$ref`
//! resolved, a `oneOf` of text constants as a plain `enum` whose values the
//! description names, and an `anyOf` of one type and `null` as that type.

use rmcp::model::JsonObject;
use serde_json::{Map, Value};

/// The schema with references resolved and value lists flattened.
pub fn flatten(schema: &JsonObject) -> JsonObject {
    let definitions = match schema.get("$defs") {
        Some(Value::Object(definitions)) => definitions.clone(),
        _ => Map::new(),
    };
    match resolve(Value::Object(schema.clone()), &definitions) {
        Value::Object(flat) => flat,
        _ => unreachable!("an object stays an object"),
    }
}

/// Removes a property, from `properties` and from `required`.
pub fn hide_property(schema: &mut JsonObject, name: &str) {
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        properties.remove(name);
    }
    if let Some(Value::Array(required)) = schema.get_mut("required") {
        required.retain(|entry| entry != name);
    }
}

fn resolve(node: Value, definitions: &Map<String, Value>) -> Value {
    let mut node = match node {
        Value::Array(items) => {
            return Value::Array(
                items
                    .into_iter()
                    .map(|item| resolve(item, definitions))
                    .collect(),
            );
        }
        Value::Object(node) => node,
        other => return other,
    };

    if let Some(Value::String(reference)) = node.remove("$ref") {
        let name = reference.rsplit('/').next().unwrap_or_default();
        if let Some(Value::Object(definition)) = definitions.get(name) {
            // The node's own keywords stay, such as the description of a
            // field; the definition's description follows it.
            let own = node.remove("description");
            for (key, value) in definition {
                node.entry(key.clone()).or_insert_with(|| value.clone());
            }
            if let Some(own) = own
                && let Some(Value::String(referenced)) = node.insert("description".into(), own)
            {
                join_description(&mut node, &referenced);
            }
        }
    }
    let mut node: Map<String, Value> = node
        .into_iter()
        .filter(|(key, _)| key != "$defs")
        .map(|(key, value)| (key, resolve(value, definitions)))
        .collect();

    // Only text constants: the enum is typed as string.
    let constants = match node.get("oneOf") {
        Some(Value::Array(options))
            if options
                .iter()
                .all(|option| option.get("const").is_some_and(Value::is_string)) =>
        {
            Some(options.clone())
        }
        _ => None,
    };
    if let Some(options) = constants {
        node.remove("oneOf");
        node.insert("type".into(), "string".into());
        node.insert(
            "enum".into(),
            options
                .iter()
                .map(|option| option["const"].clone())
                .collect(),
        );
        let named: Vec<String> = options
            .iter()
            .filter_map(|option| {
                let description = option.get("description")?.as_str()?;
                Some(format!("{}: {description}", option["const"].as_str()?))
            })
            .collect();
        join_description(&mut node, &named.join("; "));
    }

    let single = match node.get("anyOf") {
        Some(Value::Array(options)) if options.len() == 2 => {
            let concrete: Vec<&Value> = options
                .iter()
                .filter(|option| option.get("type") != Some(&Value::from("null")))
                .collect();
            match concrete.as_slice() {
                [Value::Object(inner)] => Some(inner.clone()),
                _ => None,
            }
        }
        _ => None,
    };
    if let Some(inner) = single {
        node.remove("anyOf");
        let outer = node.remove("description");
        let mut merged = inner;
        let inner_description = merged.remove("description");
        merged.extend(node);
        let description: Vec<&str> = [&outer, &inner_description]
            .into_iter()
            .filter_map(|text| text.as_ref()?.as_str())
            .filter(|text| !text.is_empty())
            .collect();
        if !description.is_empty() {
            merged.insert("description".into(), description.join(" ").into());
        }
        node = merged;
    }

    Value::Object(node)
}

/// Appends `addition` to the node's description, with a space between.
fn join_description(node: &mut Map<String, Value>, addition: &str) {
    let existing = node
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let joined: Vec<&str> = [existing, addition]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect();
    if !joined.is_empty() {
        node.insert("description".into(), joined.join(" ").into());
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn object(value: Value) -> JsonObject {
        match value {
            Value::Object(object) => object,
            _ => panic!("not an object"),
        }
    }

    #[test]
    fn an_optional_enum_becomes_a_plain_enum_with_described_values() {
        let schema = object(json!({
            "$defs": {"Priority": {
                "description": "Notification priority.",
                "oneOf": [
                    {"const": "low", "description": "Quiet.", "type": "string"},
                    {"const": "high", "description": "Loud.", "type": "string"},
                ],
            }},
            "type": "object",
            "properties": {
                "priority": {
                    "anyOf": [{"$ref": "#/$defs/Priority"}, {"type": "null"}],
                    "default": null,
                    "description": "Priority; omit for normal.",
                },
                "message": {"type": "string", "description": "Text."},
            },
            "required": ["message"],
        }));
        assert_eq!(
            Value::Object(flatten(&schema)),
            json!({
                "type": "object",
                "properties": {
                    "priority": {
                        "type": "string",
                        "enum": ["low", "high"],
                        "default": null,
                        "description": "Priority; omit for normal. Notification priority. \
                                        low: Quiet.; high: Loud.",
                    },
                    "message": {"type": "string", "description": "Text."},
                },
                "required": ["message"],
            })
        );
    }

    #[test]
    fn a_reference_keeps_the_fields_own_description() {
        let schema = object(json!({
            "$defs": {"Priority": {
                "description": "Notification priority.",
                "oneOf": [
                    {"const": "low", "description": "Quiet.", "type": "string"},
                    {"const": "high", "description": "Loud.", "type": "string"},
                ],
            }},
            "type": "object",
            "properties": {
                "priority": {"$ref": "#/$defs/Priority", "description": "Priority of this call."},
            },
        }));
        assert_eq!(
            Value::Object(flatten(&schema)),
            json!({
                "type": "object",
                "properties": {
                    "priority": {
                        "type": "string",
                        "enum": ["low", "high"],
                        "description": "Priority of this call. Notification priority. \
                                        low: Quiet.; high: Loud.",
                    },
                },
            })
        );
    }

    #[test]
    fn other_schemas_stay_as_they_are() {
        let schema = object(json!({
            "type": "object",
            "properties": {
                "title": {"type": ["string", "null"], "maxLength": 250},
                "either": {"anyOf": [{"type": "string"}, {"type": "integer"}]},
                "level": {"oneOf": [{"const": 1}, {"const": 2}]},
            },
        }));
        assert_eq!(flatten(&schema), schema);
    }

    #[test]
    fn a_hidden_property_is_gone_from_properties_and_required() {
        let mut schema = object(json!({
            "type": "object",
            "properties": {"message": {"type": "string"}, "sender": {"type": "string"}},
            "required": ["message", "sender"],
        }));
        hide_property(&mut schema, "sender");
        assert_eq!(
            Value::Object(schema),
            json!({
                "type": "object",
                "properties": {"message": {"type": "string"}},
                "required": ["message"],
            })
        );
    }
}
