use biome_json_formatter::context::JsonFormatOptions;
use biome_json_parser::{parse_json, JsonParserOptions};
use biome_service::Configuration;
use schemars::schema_for;
use serde_json::to_string;
use xtask::{project_root, Mode, Result};
use xtask_codegen::update;

pub(crate) fn generate_configuration_schema(mode: Mode) -> Result<()> {
    let schema_path_npm = project_root().join("packages/@biomejs/biome/configuration_schema.json");

    let schema = schema_for!(Configuration);
    let json_schema = to_string(&schema)?;

    let parsed = parse_json(&json_schema, JsonParserOptions::default());
    let formatted =
        biome_json_formatter::format_node(JsonFormatOptions::default(), &parsed.syntax())
            .unwrap()
            .print()
            .unwrap();

    update(&schema_path_npm, formatted.as_code(), &mode)?;

    Ok(())
}
