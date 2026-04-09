#![cfg(feature = "tree-sitter-ast")]

//! Tree-sitter helpers shared across providers.

use std::collections::HashSet;
use std::sync::LazyLock;

use tree_sitter::{Language, Parser, Query, Tree};

/// Wrapper around the tree-sitter parser with multi-language dispatch.
#[derive(Default)]
pub struct TsParser {
    parser: Parser,
}

impl TsParser {
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
        }
    }

    pub fn parse(&mut self, language: &str, source: &str) -> Option<Tree> {
        let lang = TsLanguageKind::from_name(language)?;
        let language = lang.ts_language();
        self.parser.set_language(&language).ok()?;
        self.parser.parse(source, None)
    }
}

/// Supported tree-sitter languages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TsLanguageKind {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Jsx,
    Python,
    Go,
}

impl TsLanguageKind {
    pub fn from_name(language: &str) -> Option<Self> {
        match language {
            "Rust" => Some(Self::Rust),
            "TypeScript" => Some(Self::TypeScript),
            "TSX" => Some(Self::Tsx),
            "JavaScript" => Some(Self::JavaScript),
            "JSX" => Some(Self::Jsx),
            "Python" => Some(Self::Python),
            "Go" => Some(Self::Go),
            _ => None,
        }
    }

    pub fn ts_language(self) -> Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE,
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX,
            Self::JavaScript => tree_sitter_javascript::LANGUAGE,
            Self::Jsx => tree_sitter_javascript::LANGUAGE,
            Self::Python => tree_sitter_python::LANGUAGE,
            Self::Go => tree_sitter_go::LANGUAGE,
        }
        .into()
    }
}

/// Return true if the AST node kind should be materialized.
pub fn is_significant_node(lang: TsLanguageKind, kind: &str) -> bool {
    let set = match lang {
        TsLanguageKind::Rust => &*RUST_SIGNIFICANT_KINDS,
        TsLanguageKind::TypeScript | TsLanguageKind::Tsx => &*TS_SIGNIFICANT_KINDS,
        TsLanguageKind::JavaScript | TsLanguageKind::Jsx => &*JS_SIGNIFICANT_KINDS,
        TsLanguageKind::Python => &*PY_SIGNIFICANT_KINDS,
        TsLanguageKind::Go => &*GO_SIGNIFICANT_KINDS,
    };
    set.contains(kind)
}

static RUST_SIGNIFICANT_KINDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        // Definitions
        "function_item",
        "struct_item",
        "enum_item",
        "trait_item",
        "impl_item",
        "type_item",
        "mod_item",
        "const_item",
        "static_item",
        "macro_definition",
        // Imports
        "use_declaration",
        // Control flow
        "if_expression",
        "match_expression",
        "for_expression",
        "while_expression",
        "loop_expression",
        "return_expression",
        // Declarations
        "let_declaration",
        // Calls
        "call_expression",
        // Literals
        "string_literal",
    ])
});

static TS_SIGNIFICANT_KINDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "function_declaration",
        "method_definition",
        "class_declaration",
        "interface_declaration",
        "type_alias_declaration",
        "enum_declaration",
        "arrow_function",
        "lexical_declaration",
        "variable_declaration",
        "import_statement",
        "export_statement",
        "if_statement",
        "for_statement",
        "for_in_statement",
        "while_statement",
        "switch_statement",
        "return_statement",
        "try_statement",
        "catch_clause",
        "throw_statement",
        "call_expression",
        "new_expression",
        "string",
    ])
});

static JS_SIGNIFICANT_KINDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "function_declaration",
        "method_definition",
        "class_declaration",
        "lexical_declaration",
        "variable_declaration",
        "import_statement",
        "export_statement",
        "if_statement",
        "for_statement",
        "for_in_statement",
        "while_statement",
        "switch_statement",
        "return_statement",
        "try_statement",
        "catch_clause",
        "throw_statement",
        "call_expression",
        "new_expression",
        "string",
    ])
});

static PY_SIGNIFICANT_KINDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "function_definition",
        "class_definition",
        "decorated_definition",
        "import_statement",
        "import_from_statement",
        "assignment",
        "global_statement",
        "if_statement",
        "for_statement",
        "while_statement",
        "try_statement",
        "except_clause",
        "with_statement",
        "return_statement",
        "raise_statement",
        "call",
        "attribute",
        "string",
    ])
});

static GO_SIGNIFICANT_KINDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "function_declaration",
        "method_declaration",
        "type_declaration",
        "type_spec",
        "import_declaration",
        "if_statement",
        "for_statement",
        "switch_statement",
        "select_statement",
        "return_statement",
        "defer_statement",
        "go_statement",
        "var_declaration",
        "const_declaration",
        "short_var_declaration",
        "call_expression",
        "string_literal",
    ])
});

const RUST_SYMBOL_QUERY_SRC: &str = r#"
(function_item) @function.definition
(struct_item) @struct.definition
(enum_item) @enum.definition
(trait_item) @trait.definition
(type_item) @type_alias.definition
(mod_item) @module.definition
(const_item) @const.definition
(static_item) @static.definition
(macro_definition) @macro.definition
(impl_item) @impl.definition
"#;

const TS_SYMBOL_QUERY_SRC: &str = r#"
(function_declaration) @function.definition
(method_definition) @method.definition
(class_declaration) @class.definition
(interface_declaration) @interface.definition
(type_alias_declaration) @type_alias.definition
(enum_declaration) @enum.definition
"#;

const JS_SYMBOL_QUERY_SRC: &str = r#"
(function_declaration) @function.definition
(method_definition) @method.definition
(class_declaration) @class.definition
"#;

const PY_SYMBOL_QUERY_SRC: &str = r#"
(function_definition) @function.definition
(class_definition) @class.definition
(decorated_definition) @decorated.definition
"#;

const GO_SYMBOL_QUERY_SRC: &str = r#"
(function_declaration) @function.definition
(method_declaration) @method.definition
(type_declaration) @type.definition
"#;

const RUST_IMPORT_QUERY_SRC: &str = r#"
(use_declaration) @import.use
"#;

const TS_IMPORT_QUERY_SRC: &str = r#"
(import_statement) @import.statement
(export_statement) @export.statement
"#;

const JS_IMPORT_QUERY_SRC: &str = r#"
(import_statement) @import.statement
(export_statement) @export.statement
"#;

const PY_IMPORT_QUERY_SRC: &str = r#"
(import_statement) @import.statement
(import_from_statement) @import.from
"#;

const GO_IMPORT_QUERY_SRC: &str = r#"
(import_declaration) @import.declaration
"#;

static RUST_SYMBOL_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = TsLanguageKind::Rust.ts_language();
    Query::new(&language, RUST_SYMBOL_QUERY_SRC).expect("rust symbol query")
});

static TS_SYMBOL_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = TsLanguageKind::TypeScript.ts_language();
    Query::new(&language, TS_SYMBOL_QUERY_SRC).expect("ts symbol query")
});

static JS_SYMBOL_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = TsLanguageKind::JavaScript.ts_language();
    Query::new(&language, JS_SYMBOL_QUERY_SRC).expect("js symbol query")
});

static PY_SYMBOL_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = TsLanguageKind::Python.ts_language();
    Query::new(&language, PY_SYMBOL_QUERY_SRC).expect("py symbol query")
});

static GO_SYMBOL_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = TsLanguageKind::Go.ts_language();
    Query::new(&language, GO_SYMBOL_QUERY_SRC).expect("go symbol query")
});

static RUST_IMPORT_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = TsLanguageKind::Rust.ts_language();
    Query::new(&language, RUST_IMPORT_QUERY_SRC).expect("rust import query")
});

static TS_IMPORT_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = TsLanguageKind::TypeScript.ts_language();
    Query::new(&language, TS_IMPORT_QUERY_SRC).expect("ts import query")
});

static JS_IMPORT_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = TsLanguageKind::JavaScript.ts_language();
    Query::new(&language, JS_IMPORT_QUERY_SRC).expect("js import query")
});

static PY_IMPORT_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = TsLanguageKind::Python.ts_language();
    Query::new(&language, PY_IMPORT_QUERY_SRC).expect("py import query")
});

static GO_IMPORT_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language = TsLanguageKind::Go.ts_language();
    Query::new(&language, GO_IMPORT_QUERY_SRC).expect("go import query")
});

/// Return the query that surfaces symbol definitions for the requested language.
pub fn symbol_query(lang: TsLanguageKind) -> Option<&'static Query> {
    match lang {
        TsLanguageKind::Rust => Some(&*RUST_SYMBOL_QUERY),
        TsLanguageKind::TypeScript | TsLanguageKind::Tsx => Some(&*TS_SYMBOL_QUERY),
        TsLanguageKind::JavaScript | TsLanguageKind::Jsx => Some(&*JS_SYMBOL_QUERY),
        TsLanguageKind::Python => Some(&*PY_SYMBOL_QUERY),
        TsLanguageKind::Go => Some(&*GO_SYMBOL_QUERY),
    }
}

/// Return the import query for a language, if one exists.
pub fn import_query(lang: TsLanguageKind) -> Option<&'static Query> {
    match lang {
        TsLanguageKind::Rust => Some(&*RUST_IMPORT_QUERY),
        TsLanguageKind::TypeScript | TsLanguageKind::Tsx => Some(&*TS_IMPORT_QUERY),
        TsLanguageKind::JavaScript | TsLanguageKind::Jsx => Some(&*JS_IMPORT_QUERY),
        TsLanguageKind::Python => Some(&*PY_IMPORT_QUERY),
        TsLanguageKind::Go => Some(&*GO_IMPORT_QUERY),
    }
}
