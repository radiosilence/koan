//! Compiles the web UI's scripts, `scripts/*.ts`, into `OUT_DIR/*.js`, which
//! the crate embeds. Only the types are removed: the scripts are written in
//! erasable TypeScript for the browsers they run in, so nothing is lowered or
//! minified. tsc typechecks them (`just js`); this only strips.

use std::path::Path;

use oxc::allocator::Allocator;
use oxc::codegen::{Codegen, CodegenOptions, IndentChar};
use oxc::diagnostics::OxcDiagnostic;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{TransformOptions, Transformer};

fn main() {
    let src = Path::new("scripts");
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    println!("cargo:rerun-if-changed=scripts");
    let mut names: Vec<_> = std::fs::read_dir(src)
        .expect("scripts/ is readable")
        .map(|e| {
            e.expect("scripts/ is readable")
                .file_name()
                .into_string()
                .expect("UTF-8 names")
        })
        .filter(|n| n.ends_with(".ts") && !n.ends_with(".d.ts"))
        .collect();
    names.sort();
    for name in names {
        let path = src.join(&name);
        println!("cargo:rerun-if-changed={}", path.display());
        let ts = std::fs::read_to_string(&path).expect("a script is readable");
        let js = strip(&path, &ts).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(Path::new(&out).join(name.replace(".ts", ".js")), js)
            .expect("OUT_DIR is writable");
    }
}

fn strip(path: &Path, ts: &str) -> Result<String, String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, ts, SourceType::ts().with_module(true)).parse();
    report(path, ts, &parsed.diagnostics)?;
    let mut program = parsed.program;
    let scoping = SemanticBuilder::new()
        .build(&program)
        .semantic
        .into_scoping();
    let transformed = Transformer::new(&allocator, path, &TransformOptions::default())
        .build_with_scoping(scoping, &mut program);
    report(path, ts, &transformed.diagnostics)?;
    let options = CodegenOptions {
        indent_char: IndentChar::Space,
        indent_width: 2,
        ..CodegenOptions::default()
    };
    Ok(Codegen::new().with_options(options).build(&program).code)
}

/// The errors as `path:line:column: message`, one per line.
fn report(path: &Path, ts: &str, errors: &[OxcDiagnostic]) -> Result<(), String> {
    if errors.is_empty() {
        return Ok(());
    }
    let lines = errors.iter().map(|e| {
        let at = e.labels.first().map_or(0, |l| l.offset() as usize);
        let before = &ts[..at.min(ts.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
        format!("{}:{line}:{column}: {e}", path.display())
    });
    Err(lines.collect::<Vec<_>>().join("\n"))
}
