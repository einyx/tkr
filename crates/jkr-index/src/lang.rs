//! Language detection + per-language tree-sitter queries.
//!
//! Each query captures a "parent" node (whose range gives line_start/line_end)
//! and a `.name` sub-capture (the identifier). Optional `.sig` sub-captures
//! provide a signature one-liner; absent → falls back to first line of the
//! parent node's text.

use tree_sitter::Language;

#[derive(Debug, Clone, Copy)]
pub struct LangSpec {
    pub name: &'static str,
    pub language: fn() -> Language,
    pub query: &'static str,
    /// Captures call sites. The `@callee.name` sub-capture is the callee
    /// identifier as written at the call site (unresolved).
    pub calls_query: &'static str,
}

pub fn detect(path: &str) -> Option<LangSpec> {
    let ext = path.rsplit_once('.').map(|(_, e)| e)?;
    Some(match ext {
        "rs" => RUST,
        "py" => PYTHON,
        "go" => GO,
        "ts" | "tsx" => TYPESCRIPT,
        "js" | "jsx" | "mjs" | "cjs" => JAVASCRIPT,
        "java" => JAVA,
        "c" | "h" => C,
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => CPP,
        "rb" => RUBY,
        _ => return None,
    })
}

/// Map a parser-emitted capture name (e.g. `fn`, `struct`, `method`) to a
/// canonical kind. Free-form strings stored in the DB; this is the single
/// chokepoint that prevents `Function` vs `function` drift across parsers.
pub fn canonical_kind(raw: &str) -> &'static str {
    match raw {
        "fn" | "func" | "function" | "function_declaration" => "function",
        "method" | "method_definition" => "method",
        "struct" => "struct",
        "enum" => "enum",
        "trait" | "interface" => "trait",
        "impl" => "impl",
        "class" | "class_declaration" => "class",
        "mod" | "module" => "module",
        "const" | "constant" => "const",
        "static" => "static",
        "type" | "type_alias" => "type",
        "var" | "variable" => "var",
        _ => "other",
    }
}

pub const RUST: LangSpec = LangSpec {
    name: "rust",
    language: || tree_sitter_rust::LANGUAGE.into(),
    query: r#"
        (function_item name: (identifier) @fn.name) @fn
        (struct_item name: (type_identifier) @struct.name) @struct
        (enum_item name: (type_identifier) @enum.name) @enum
        (trait_item name: (type_identifier) @trait.name) @trait
        (impl_item type: (type_identifier) @impl.name) @impl
        (mod_item name: (identifier) @mod.name) @mod
        (const_item name: (identifier) @const.name) @const
        (static_item name: (identifier) @static.name) @static
        (type_item name: (type_identifier) @type.name) @type
    "#,
    calls_query: r#"
        (call_expression function: (identifier) @callee.name) @call
        (call_expression function: (field_expression field: (field_identifier) @callee.name)) @call
        (call_expression function: (scoped_identifier name: (identifier) @callee.name)) @call
    "#,
};

pub const PYTHON: LangSpec = LangSpec {
    name: "python",
    language: || tree_sitter_python::LANGUAGE.into(),
    query: r#"
        (function_definition name: (identifier) @function.name) @function
        (class_definition name: (identifier) @class.name) @class
    "#,
    calls_query: r#"
        (call function: (identifier) @callee.name) @call
        (call function: (attribute attribute: (identifier) @callee.name)) @call
    "#,
};

pub const GO: LangSpec = LangSpec {
    name: "go",
    language: || tree_sitter_go::LANGUAGE.into(),
    query: r#"
        (function_declaration name: (identifier) @function.name) @function
        (method_declaration name: (field_identifier) @method.name) @method
        (type_declaration (type_spec name: (type_identifier) @type.name)) @type
    "#,
    calls_query: r#"
        (call_expression function: (identifier) @callee.name) @call
        (call_expression function: (selector_expression field: (field_identifier) @callee.name)) @call
    "#,
};

pub const TYPESCRIPT: LangSpec = LangSpec {
    name: "typescript",
    language: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
    query: r#"
        (function_declaration name: (identifier) @function.name) @function
        (class_declaration name: (type_identifier) @class.name) @class
        (interface_declaration name: (type_identifier) @interface.name) @interface
        (method_definition name: (property_identifier) @method.name) @method
    "#,
    calls_query: r#"
        (call_expression function: (identifier) @callee.name) @call
        (call_expression function: (member_expression property: (property_identifier) @callee.name)) @call
    "#,
};

pub const JAVASCRIPT: LangSpec = LangSpec {
    name: "javascript",
    language: || tree_sitter_javascript::LANGUAGE.into(),
    query: r#"
        (function_declaration name: (identifier) @function.name) @function
        (class_declaration name: (identifier) @class.name) @class
        (method_definition name: (property_identifier) @method.name) @method
    "#,
    calls_query: r#"
        (call_expression function: (identifier) @callee.name) @call
        (call_expression function: (member_expression property: (property_identifier) @callee.name)) @call
    "#,
};

pub const JAVA: LangSpec = LangSpec {
    name: "java",
    language: || tree_sitter_java::LANGUAGE.into(),
    query: r#"
        (class_declaration name: (identifier) @class.name) @class
        (interface_declaration name: (identifier) @interface.name) @interface
        (enum_declaration name: (identifier) @enum.name) @enum
        (method_declaration name: (identifier) @method.name) @method
        (constructor_declaration name: (identifier) @method.name) @method
    "#,
    calls_query: r#"
        (method_invocation name: (identifier) @callee.name) @call
        (object_creation_expression type: (type_identifier) @callee.name) @call
    "#,
};

pub const C: LangSpec = LangSpec {
    name: "c",
    language: || tree_sitter_c::LANGUAGE.into(),
    query: r#"
        (function_definition declarator: (function_declarator declarator: (identifier) @function.name)) @function
        (struct_specifier name: (type_identifier) @struct.name) @struct
        (enum_specifier name: (type_identifier) @enum.name) @enum
        (type_definition declarator: (type_identifier) @type.name) @type
    "#,
    calls_query: r#"
        (call_expression function: (identifier) @callee.name) @call
        (call_expression function: (field_expression field: (field_identifier) @callee.name)) @call
    "#,
};

pub const CPP: LangSpec = LangSpec {
    name: "cpp",
    language: || tree_sitter_cpp::LANGUAGE.into(),
    query: r#"
        (function_definition declarator: (function_declarator declarator: (identifier) @function.name)) @function
        (function_definition declarator: (function_declarator declarator: (qualified_identifier name: (identifier) @function.name))) @function
        (class_specifier name: (type_identifier) @class.name) @class
        (struct_specifier name: (type_identifier) @struct.name) @struct
        (enum_specifier name: (type_identifier) @enum.name) @enum
        (namespace_definition name: (namespace_identifier) @module.name) @module
    "#,
    calls_query: r#"
        (call_expression function: (identifier) @callee.name) @call
        (call_expression function: (field_expression field: (field_identifier) @callee.name)) @call
        (call_expression function: (qualified_identifier name: (identifier) @callee.name)) @call
    "#,
};

pub const RUBY: LangSpec = LangSpec {
    name: "ruby",
    language: || tree_sitter_ruby::LANGUAGE.into(),
    query: r#"
        (class name: (constant) @class.name) @class
        (module name: (constant) @module.name) @module
        (method name: (identifier) @method.name) @method
        (singleton_method name: (identifier) @method.name) @method
    "#,
    calls_query: r#"
        (call method: (identifier) @callee.name) @call
    "#,
};
