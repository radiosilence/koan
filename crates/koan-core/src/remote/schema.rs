//! JSON Schema for the protocol's messages, generated from these types into
//! `docs/reference/protocol/schema/`. The test fails when the committed files
//! differ from what the types produce; `just protocol-schema` writes them.

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
    let mut json = serde_json::to_string_pretty(&schema).unwrap();
    json.push('\n');
    json
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
