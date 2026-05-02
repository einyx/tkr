//! Source-file outline extraction. Tree-sitter for Rust today; other
//! languages fall back to a header-only summary (file size + first
//! non-blank line).

use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::Path;
use tree_sitter::{Parser, Query, QueryCursor};

/// One symbol entry in an outline. Line numbers are 1-based.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub kind: &'static str,
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// Render a file's outline as a compact text block. Lines look like:
///   `fn   parse_request                  L42-L88`
///   `struct Server                       L120-L145`
pub fn render_outline(path: &Path) -> Result<String> {
    let path = path
        .canonicalize()
        .with_context(|| format!("canonicalize {}", path.display()))?;
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let total_lines = bytes.iter().filter(|b| **b == b'\n').count() + 1;

    let symbols = match path.extension().and_then(|s| s.to_str()) {
        Some("rs") => rust_outline(&bytes)?,
        Some("py") => language_outline(&bytes, tree_sitter_python::language(), PYTHON_QUERY)?,
        Some("go") => language_outline(&bytes, tree_sitter_go::language(), GO_QUERY)?,
        Some("ts" | "tsx") => language_outline(
            &bytes,
            tree_sitter_typescript::language_typescript(),
            TS_QUERY,
        )?,
        Some("js" | "jsx" | "mjs" | "cjs") => language_outline(
            &bytes,
            tree_sitter_javascript::language(),
            JS_QUERY,
        )?,
        _ => Vec::new(),
    };

    let mut out = String::new();
    out.push_str(&format!(
        "outline: {} ({} lines, {} bytes)\n",
        path.display(),
        total_lines,
        bytes.len()
    ));
    if symbols.is_empty() {
        // Fallback: header-only summary.
        let preview = std::str::from_utf8(&bytes)
            .unwrap_or("(binary)")
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        out.push_str(&format!("(no outline; first line: {preview:?})\n"));
        out.push_str("hint: pass --offset/--limit to native Read for partial views.\n");
        return Ok(out);
    }

    out.push_str(&format!("\n{} symbols:\n", symbols.len()));
    for s in &symbols {
        out.push_str(&format!(
            "  {:<8} {:<40} L{}-L{}\n",
            s.kind, s.name, s.start_line, s.end_line
        ));
    }
    Ok(out)
}

fn rust_outline(source: &[u8]) -> Result<Vec<Symbol>> {
    let mut parser = Parser::new();
    parser
        .set_language(tree_sitter_rust::language())
        .map_err(|e| anyhow!("set tree-sitter-rust: {e}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("parse failed"))?;

    // Capture the items we care about. The query names map to Symbol::kind.
    let query_str = r#"
        (function_item name: (identifier) @fn.name) @fn
        (struct_item name: (type_identifier) @struct.name) @struct
        (enum_item name: (type_identifier) @enum.name) @enum
        (trait_item name: (type_identifier) @trait.name) @trait
        (impl_item type: (type_identifier) @impl.name) @impl
        (mod_item name: (identifier) @mod.name) @mod
        (const_item name: (identifier) @const.name) @const
        (static_item name: (identifier) @static.name) @static
        (type_item name: (type_identifier) @type.name) @type
    "#;
    let query = Query::new(tree_sitter_rust::language(), query_str)
        .map_err(|e| anyhow!("compile query: {e}"))?;

    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    let capture_names = query.capture_names();
    for m in cursor.matches(&query, tree.root_node(), source) {
        // Each match has a parent capture (e.g. @fn) and a name capture (e.g. @fn.name).
        // The parent capture gives the line range; the name capture gives the symbol name.
        let mut kind: Option<&'static str> = None;
        let mut start_line = 0usize;
        let mut end_line = 0usize;
        let mut name: Option<String> = None;
        for cap in m.captures {
            let cap_name = capture_names[cap.index as usize].as_str();
            if cap_name.contains('.') {
                // It's a `.name` sub-capture.
                let bytes = &source[cap.node.byte_range()];
                if let Ok(s) = std::str::from_utf8(bytes) {
                    name = Some(s.to_string());
                }
            } else {
                // Parent capture — gives kind + range.
                kind = Some(static_kind(cap_name));
                start_line = cap.node.start_position().row + 1;
                end_line = cap.node.end_position().row + 1;
            }
        }
        if let (Some(kind), Some(name)) = (kind, name) {
            out.push(Symbol {
                kind,
                name,
                start_line,
                end_line,
            });
        }
    }
    out.sort_by_key(|s| s.start_line);
    Ok(out)
}

/// Generic dispatch: pick a language + query, run the same capture-name
/// scheme as `rust_outline`. Capture names without a `.` are treated as
/// kind labels; `.name` captures provide the symbol name.
fn language_outline(
    source: &[u8],
    language: tree_sitter::Language,
    query_str: &str,
) -> Result<Vec<Symbol>> {
    let mut parser = Parser::new();
    parser
        .set_language(language)
        .map_err(|e| anyhow!("set_language: {e}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("parse failed"))?;
    let query = Query::new(language, query_str)
        .map_err(|e| anyhow!("compile query: {e}"))?;
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    let capture_names = query.capture_names();
    for m in cursor.matches(&query, tree.root_node(), source) {
        let mut kind: Option<&'static str> = None;
        let mut start_line = 0usize;
        let mut end_line = 0usize;
        let mut name: Option<String> = None;
        for cap in m.captures {
            let cap_name = capture_names[cap.index as usize].as_str();
            if cap_name.contains('.') {
                if let Ok(s) = std::str::from_utf8(&source[cap.node.byte_range()]) {
                    name = Some(s.to_string());
                }
            } else {
                kind = Some(static_kind(cap_name));
                start_line = cap.node.start_position().row + 1;
                end_line = cap.node.end_position().row + 1;
            }
        }
        if let (Some(kind), Some(name)) = (kind, name) {
            out.push(Symbol {
                kind,
                name,
                start_line,
                end_line,
            });
        }
    }
    out.sort_by_key(|s| s.start_line);
    Ok(out)
}

const PYTHON_QUERY: &str = r#"
    (function_definition name: (identifier) @fn.name) @fn
    (class_definition name: (identifier) @class.name) @class
"#;

const GO_QUERY: &str = r#"
    (function_declaration name: (identifier) @fn.name) @fn
    (method_declaration name: (field_identifier) @fn.name) @fn
    (type_declaration (type_spec name: (type_identifier) @type.name)) @type
"#;

// TypeScript and JavaScript share most of their structure; the
// typescript grammar adds interfaces/types over the JS query.
const TS_QUERY: &str = r#"
    (function_declaration name: (identifier) @fn.name) @fn
    (method_definition name: (property_identifier) @fn.name) @fn
    (class_declaration name: (type_identifier) @class.name) @class
    (interface_declaration name: (type_identifier) @interface.name) @interface
    (type_alias_declaration name: (type_identifier) @type.name) @type
    (enum_declaration name: (identifier) @enum.name) @enum
"#;

const JS_QUERY: &str = r#"
    (function_declaration name: (identifier) @fn.name) @fn
    (method_definition name: (property_identifier) @fn.name) @fn
    (class_declaration name: (identifier) @class.name) @class
"#;

fn static_kind(s: &str) -> &'static str {
    match s {
        "fn" => "fn",
        "struct" => "struct",
        "enum" => "enum",
        "trait" => "trait",
        "impl" => "impl",
        "mod" => "mod",
        "const" => "const",
        "static" => "static",
        "type" => "type",
        "class" => "class",
        "interface" => "iface",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rust_outline_extracts_top_level_items() {
        let src = br#"
            //! header comment
            pub struct Foo { x: u32 }
            pub fn bar(x: u32) -> u32 { x + 1 }
            pub enum Color { Red, Green }
            pub trait Shape { fn area(&self) -> f64; }
            pub mod inner { pub fn helper() {} }
        "#;
        let symbols = rust_outline(src).unwrap();
        let kinds: Vec<&str> = symbols.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&"struct"));
        assert!(kinds.contains(&"fn"));
        assert!(kinds.contains(&"enum"));
        assert!(kinds.contains(&"trait"));
        assert!(kinds.contains(&"mod"));
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"bar"));
    }

    #[test]
    fn render_outline_produces_text_summary() {
        let mut tmp = tempfile_write(
            "test_outline.rs",
            r#"
pub fn alpha() {}
pub struct Beta { x: u32 }
"#,
        );
        let out = render_outline(&tmp.path).unwrap();
        assert!(out.contains("symbols"));
        assert!(out.contains("alpha"));
        assert!(out.contains("Beta"));
        assert!(out.contains("L"));
        tmp.cleanup();
    }

    #[test]
    fn python_outline_extracts_functions_and_classes() {
        let tmp = tempfile_write(
            "test_python.py",
            "def alpha():\n    pass\n\nclass Beta:\n    def gamma(self):\n        pass\n",
        );
        let out = render_outline(&tmp.path).unwrap();
        assert!(out.contains("alpha"), "{out}");
        assert!(out.contains("Beta"), "{out}");
        assert!(out.contains("class") || out.contains("fn"));
    }

    #[test]
    fn go_outline_extracts_funcs_and_types() {
        let tmp = tempfile_write(
            "test_go.go",
            "package x\n\ntype Foo struct{}\n\nfunc bar() int { return 1 }\n",
        );
        let out = render_outline(&tmp.path).unwrap();
        assert!(out.contains("bar"), "{out}");
        assert!(out.contains("Foo"), "{out}");
    }

    #[test]
    fn typescript_outline_extracts_class_and_iface() {
        let tmp = tempfile_write(
            "test_ts.ts",
            "interface Shape { area(): number; }\nclass Circle implements Shape { area() { return 0; } }\n",
        );
        let out = render_outline(&tmp.path).unwrap();
        assert!(out.contains("Shape"), "{out}");
        assert!(out.contains("Circle"), "{out}");
    }

    #[test]
    fn javascript_outline_extracts_funcs_and_classes() {
        let tmp = tempfile_write(
            "test_js.js",
            "function foo() { return 1; }\nclass Bar { baz() { return 2; } }\n",
        );
        let out = render_outline(&tmp.path).unwrap();
        assert!(out.contains("foo"), "{out}");
        assert!(out.contains("Bar"), "{out}");
    }

    #[test]
    fn unsupported_extension_falls_back_to_header() {
        let tmp = tempfile_write("plain.txt", "first line\nsecond line\n");
        let out = render_outline(&tmp.path).unwrap();
        assert!(out.contains("(no outline"));
        assert!(out.contains("first line"));
    }

    struct TempFile {
        path: std::path::PathBuf,
    }
    impl TempFile {
        fn cleanup(self) {
            let _ = fs::remove_file(&self.path);
        }
    }
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }
    fn tempfile_write(name: &str, content: &str) -> TempFile {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "tkr-mcp-test-{}-{}",
            std::process::id(),
            name
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        TempFile { path }
    }
}
