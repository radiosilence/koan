//! JSON Schema for the protocol's messages, generated from these types into
//! `docs/reference/protocol/schema/`. The test fails when the committed files
//! differ from what the types produce; `just protocol-schema` writes them.
//!
//! The schema states shapes only. Doc comments here speak of Rust names and
//! modules, so they are left out; what a message means is on the protocol
//! pages.

use std::path::PathBuf;

use schemars::{JsonSchema, generate::SchemaSettings};

use crate::remote::acks::Envelope;
use crate::remote::link::{LinkCommand, LinkReport};
use crate::remote::nearby::ProofFrame;
use crate::remote::pair::PairMessage;

fn render<T: JsonSchema>() -> String {
    let schema = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<T>();
    let mut value = schema.to_value();
    canonical(&mut value, false);
    let mut json = serde_json::to_string_pretty(&value).unwrap();
    json.push('\n');
    json
}

/// Descriptions left out and keys in order. Sorted by hand because a
/// workspace build can turn on serde_json's `preserve_order`, under which
/// objects keep the order they were built in, or removed from, rather than
/// sorting themselves.
///
/// `names` is an object keyed by names rather than keywords (`properties`,
/// `$defs`): a field called `description` there is a field, and stays.
fn canonical(value: &mut serde_json::Value, names: bool) {
    match value {
        serde_json::Value::Object(map) => {
            if !names {
                map.remove("description");
            }
            map.sort_keys();
            for (key, child) in map.iter_mut() {
                let keyed = !names
                    && matches!(
                        key.as_str(),
                        "properties" | "patternProperties" | "$defs" | "definitions"
                    );
                canonical(child, keyed);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(|item| canonical(item, false)),
        _ => {}
    }
}

#[test]
fn a_field_called_description_is_kept() {
    let mut schema = serde_json::json!({
        "description": "a doc comment",
        "properties": {
            "description": { "description": "its doc comment", "type": "string" }
        }
    });
    canonical(&mut schema, false);
    assert_eq!(
        schema,
        serde_json::json!({ "properties": { "description": { "type": "string" } } })
    );
}

#[test]
fn committed_schemas_match_the_types() {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference/protocol/schema");
    let schemas = [
        ("link-command.json", render::<LinkCommand>()),
        ("link-report.json", render::<LinkReport>()),
        ("envelope.json", render::<Envelope>()),
        ("pair-message.json", render::<PairMessage>()),
        ("nearby-frame.json", render::<ProofFrame>()),
    ];
    if std::env::var_os("KOAN_WRITE_PROTOCOL_SCHEMA").is_some() {
        std::fs::create_dir_all(&dir).unwrap();
        for (name, json) in &schemas {
            std::fs::write(dir.join(name), json).unwrap();
        }
        return;
    }
    let stale: Vec<&str> = schemas
        .iter()
        .filter(|(name, json)| std::fs::read_to_string(dir.join(name)).ok().as_ref() != Some(json))
        .map(|(name, _)| *name)
        .collect();
    assert!(
        stale.is_empty(),
        "protocol schemas out of date: {stale:?}; run `just protocol-schema` and commit the result"
    );
}
