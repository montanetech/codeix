//! Rust symbol and text extraction.

use tree_sitter::{Node, Tree};

use crate::index::format::{ReferenceEntry, SymbolEntry, TextEntry};
use crate::parser::helpers::*;
use crate::parser::treesitter::MAX_DEPTH;

pub fn extract(
    tree: &Tree,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
    let root = tree.root_node();
    walk_node(root, source, file_path, None, symbols, texts, references, 0);
}

#[allow(clippy::too_many_arguments)]
fn walk_node(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
    references: &mut Vec<ReferenceEntry>,
    depth: usize,
) {
    // Prevent stack overflow on deeply nested code
    if depth > MAX_DEPTH {
        return;
    }

    let kind = node.kind();

    match kind {
        "function_item" => {
            extract_function(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return; // handled recursively
        }
        "struct_item" => {
            extract_struct(node, source, file_path, parent_ctx, symbols, references);
        }
        "enum_item" => {
            extract_named_symbol(node, source, file_path, "enum", parent_ctx, symbols);
        }
        "trait_item" => {
            extract_named_symbol(node, source, file_path, "interface", parent_ctx, symbols);
        }
        "type_item" => {
            extract_named_symbol(node, source, file_path, "type_alias", parent_ctx, symbols);
        }
        "mod_item" => {
            extract_named_symbol(node, source, file_path, "module", parent_ctx, symbols);
        }
        "const_item" => {
            extract_named_symbol(node, source, file_path, "constant", parent_ctx, symbols);
        }
        "static_item" => {
            extract_named_symbol(node, source, file_path, "constant", parent_ctx, symbols);
        }
        "use_declaration" => {
            extract_use(node, source, file_path, symbols, references);
        }
        "impl_item" => {
            extract_impl(node, source, file_path, symbols, texts, references, depth);
            return; // impl is handled recursively inside extract_impl
        }
        "call_expression" => {
            extract_call(node, source, file_path, parent_ctx, references);
        }
        "line_comment" | "block_comment" => {
            extract_rust_comment(node, source, file_path, parent_ctx, texts);
            return;
        }
        "string_literal" | "raw_string_literal" => {
            extract_string(node, source, file_path, parent_ctx, texts);
            return;
        }
        _ => {}
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_node(
            child,
            source,
            file_path,
            parent_ctx,
            symbols,
            texts,
            references,
            depth + 1,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_function(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
    references: &mut Vec<ReferenceEntry>,
    depth: usize,
) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let visibility = extract_visibility(node, source);
    let line = node_line_range(node);

    // Extract tokens from function body for FTS
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| extract_tokens(body, source))
        .map(|t| filter_rust_tokens(&t));

    let kind = if parent_ctx.is_some() {
        "method"
    } else {
        "function"
    };

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name
    };

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        kind,
        line,
        parent_ctx,
        tokens,
        None,
        Some(visibility),
    );

    // Extract type references from parameters
    if let Some(params) = find_child_by_field(node, "parameters") {
        extract_type_refs_from_node(params, source, file_path, Some(&full_name), references);
    }

    // Extract type references from return type
    if let Some(ret_type) = find_child_by_field(node, "return_type") {
        extract_type_refs_from_node(ret_type, source, file_path, Some(&full_name), references);
    }

    // Recurse into function body with function name as context
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            walk_node(
                child,
                source,
                file_path,
                Some(&full_name),
                symbols,
                texts,
                references,
                depth + 1,
            );
        }
    }
}

fn extract_named_symbol(
    node: Node,
    source: &[u8],
    file_path: &str,
    kind: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let visibility = extract_visibility(node, source);
    let line = node_line_range(node);

    push_symbol(
        symbols,
        file_path,
        name,
        kind,
        line,
        parent_ctx,
        None,
        None,
        Some(visibility),
    );
}

fn extract_struct(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let visibility = extract_visibility(node, source);
    let line = node_line_range(node);

    push_symbol(
        symbols,
        file_path,
        name.clone(),
        "struct",
        line,
        parent_ctx,
        None,
        None,
        Some(visibility),
    );

    // Extract type references from struct fields
    if let Some(body) = find_child_by_field(node, "body") {
        extract_type_refs_from_node(body, source, file_path, Some(&name), references);
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_impl(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
    references: &mut Vec<ReferenceEntry>,
    depth: usize,
) {
    let impl_type_name = extract_impl_type_name(node, source);
    let line = node_line_range(node);
    let visibility = extract_visibility(node, source);

    let trait_name = find_child_by_field(node, "trait").map(|n| node_text(n, source));

    let kind = if trait_name.is_some() {
        "trait_impl"
    } else {
        "impl"
    };

    // impl blocks are containers, no meaningful tokens
    push_symbol(
        symbols,
        file_path,
        impl_type_name.clone(),
        kind,
        line,
        None,
        None,
        None,
        Some(visibility),
    );

    // Walk children of the body to find methods
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            match child.kind() {
                "function_item" => {
                    extract_function(
                        child,
                        source,
                        file_path,
                        Some(&impl_type_name),
                        symbols,
                        texts,
                        references,
                        depth + 1,
                    );
                }
                "const_item" => {
                    extract_named_symbol(
                        child,
                        source,
                        file_path,
                        "constant",
                        Some(&impl_type_name),
                        symbols,
                    );
                }
                "type_item" => {
                    extract_named_symbol(
                        child,
                        source,
                        file_path,
                        "type_alias",
                        Some(&impl_type_name),
                        symbols,
                    );
                }
                _ => {
                    walk_node(
                        child,
                        source,
                        file_path,
                        Some(&impl_type_name),
                        symbols,
                        texts,
                        references,
                        depth + 1,
                    );
                }
            }
        }
    }
}

fn extract_impl_type_name(node: Node, source: &[u8]) -> String {
    if let Some(type_node) = find_child_by_field(node, "type") {
        return node_text(type_node, source);
    }
    "Unknown".to_string()
}

fn extract_use(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
    let line = node_line_range(node);
    let visibility = extract_visibility(node, source);

    if let Some(arg) = find_child_by_field(node, "argument") {
        extract_use_paths(
            arg,
            source,
            file_path,
            &line,
            &visibility,
            symbols,
            references,
        );
    }
}

fn extract_use_paths(
    node: Node,
    source: &[u8],
    file_path: &str,
    line: &[u32; 2],
    visibility: &str,
    symbols: &mut Vec<SymbolEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
    match node.kind() {
        "use_as_clause" => {
            if let Some(path_node) = find_child_by_field(node, "path") {
                let name = node_text(path_node, source);
                let alias = find_child_by_field(node, "alias").map(|n| node_text(n, source));
                push_symbol(
                    symbols,
                    file_path,
                    name.clone(),
                    "import",
                    *line,
                    None,
                    None,
                    alias,
                    Some(visibility.to_string()),
                );
                // Also record as import reference
                references.push(ReferenceEntry {
                    file: file_path.to_string(),
                    name,
                    kind: "import".to_string(),
                    line: *line,
                    caller: None,
                    project: String::new(),
                });
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_use_paths(
                    child, source, file_path, line, visibility, symbols, references,
                );
            }
        }
        "use_wildcard" | "scoped_use_list" => {
            let name = node_text(node, source);
            push_symbol(
                symbols,
                file_path,
                name.clone(),
                "import",
                *line,
                None,
                None,
                None,
                Some(visibility.to_string()),
            );
            // Also record as import reference
            references.push(ReferenceEntry {
                file: file_path.to_string(),
                name,
                kind: "import".to_string(),
                line: *line,
                caller: None,
                project: String::new(),
            });
        }
        "scoped_identifier" | "identifier" => {
            let name = node_text(node, source);
            push_symbol(
                symbols,
                file_path,
                name.clone(),
                "import",
                *line,
                None,
                None,
                None,
                Some(visibility.to_string()),
            );
            // Also record as import reference
            references.push(ReferenceEntry {
                file: file_path.to_string(),
                name,
                kind: "import".to_string(),
                line: *line,
                caller: None,
                project: String::new(),
            });
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_use_paths(
                    child, source, file_path, line, visibility, symbols, references,
                );
            }
        }
    }
}

fn extract_visibility(node: Node, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            let text = node_text(child, source);
            if text.contains("pub(crate)") || text.contains("pub(super)") || text.contains("pub(in")
            {
                return "internal".to_string();
            }
            return "public".to_string();
        }
    }
    "private".to_string()
}

/// Extract a function call as a reference.
fn extract_call(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    references: &mut Vec<ReferenceEntry>,
) {
    let line = node_line_range(node);

    // The "function" field contains the callable expression
    let Some(func) = find_child_by_field(node, "function") else {
        return;
    };

    // Extract the name of the called function
    let name = match func.kind() {
        "identifier" => node_text(func, source),
        "scoped_identifier" | "field_expression" => node_text(func, source),
        _ => return,
    };

    // Skip macros and builtins
    if is_rust_builtin_call(&name) {
        return;
    }

    references.push(ReferenceEntry {
        file: file_path.to_string(),
        name,
        kind: "call".to_string(),
        line,
        caller: parent_ctx.map(String::from),
        project: String::new(),
    });
}

/// Check if a call is to a Rust builtin/macro that we want to skip.
fn is_rust_builtin_call(name: &str) -> bool {
    let base = name.split("::").last().unwrap_or(name);
    matches!(
        base,
        "println"
            | "print"
            | "eprintln"
            | "eprint"
            | "format"
            | "write"
            | "writeln"
            | "panic"
            | "assert"
            | "assert_eq"
            | "assert_ne"
            | "debug_assert"
            | "debug_assert_eq"
            | "debug_assert_ne"
            | "unreachable"
            | "unimplemented"
            | "todo"
            | "vec"
            | "dbg"
            | "cfg"
            | "include"
            | "include_str"
            | "include_bytes"
            | "concat"
            | "stringify"
            | "env"
            | "option_env"
            | "file"
            | "line"
            | "column"
            | "module_path"
            | "Default"
            | "Clone"
            | "Copy"
            | "Drop"
            | "Some"
            | "None"
            | "Ok"
            | "Err"
    )
}

/// Extract type references from a node (parameters, return types, fields).
fn extract_type_refs_from_node(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    references: &mut Vec<ReferenceEntry>,
) {
    let mut stack = vec![node];

    while let Some(n) = stack.pop() {
        match n.kind() {
            "type_identifier" => {
                let name = node_text(n, source);
                if !is_rust_primitive_type(&name) {
                    references.push(ReferenceEntry {
                        file: file_path.to_string(),
                        name,
                        kind: "type_annotation".to_string(),
                        line: node_line_range(n),
                        caller: parent_ctx.map(String::from),
                        project: String::new(),
                    });
                }
            }
            "scoped_type_identifier" => {
                let name = node_text(n, source);
                if !is_rust_primitive_type(&name) {
                    references.push(ReferenceEntry {
                        file: file_path.to_string(),
                        name,
                        kind: "type_annotation".to_string(),
                        line: node_line_range(n),
                        caller: parent_ctx.map(String::from),
                        project: String::new(),
                    });
                }
                continue; // Don't recurse into children
            }
            "generic_type" => {
                // Extract the base type name from generic
                if let Some(type_node) = find_child_by_field(n, "type") {
                    let name = node_text(type_node, source);
                    if !is_rust_primitive_type(&name) {
                        references.push(ReferenceEntry {
                            file: file_path.to_string(),
                            name,
                            kind: "type_annotation".to_string(),
                            line: node_line_range(type_node),
                            caller: parent_ctx.map(String::from),
                            project: String::new(),
                        });
                    }
                }
                // Also extract type arguments
                if let Some(args) = find_child_by_field(n, "type_arguments") {
                    stack.push(args);
                }
                continue;
            }
            _ => {}
        }

        // Recurse into children
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
}

/// Check if a type is a Rust primitive.
fn is_rust_primitive_type(name: &str) -> bool {
    matches!(
        name,
        "i8" | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
            | "bool"
            | "char"
            | "str"
            | "Self"
    )
}

/// Rust-specific stopwords to filter from tokens.
const RUST_STOPWORDS: &[&str] = &[
    // Keywords
    "self", "Self", "crate", "mod", "pub", "mut", "ref", "let", "type", "impl", "trait", "fn",
    "where", "loop", "match", "unsafe", "async", "await", "dyn", "move", "use", "as", "Some",
    "None", "Ok", "Err", // Common std types/modules
    "std", "core", "alloc", // Very common short names in Rust
    "cx", "rx", "tx", "io", "buf", "drop",
];

/// Filter Rust-specific tokens from the extracted token string.
fn filter_rust_tokens(tokens: &str) -> String {
    tokens
        .split_whitespace()
        .filter(|t| !RUST_STOPWORDS.contains(t))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Rust-specific comment extraction (handles ///, //!, /**, etc.)
fn extract_rust_comment(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    extract_comment(node, source, file_path, parent_ctx, texts);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::treesitter::parse_file;

    fn find_sym<'a>(symbols: &'a [SymbolEntry], name: &str) -> &'a SymbolEntry {
        symbols
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("symbol not found: {name}"))
    }

    #[test]
    fn test_rust_functions() {
        let source = b"pub fn hello(name: &str) -> String {
    format!(\"Hello, {}!\", name)
}

fn private_helper() {
    println!(\"private\");
}";
        let (symbols, _texts, _refs) = parse_file(source, "rust", "test.rs").unwrap();
        assert_eq!(symbols.len(), 2);

        let hello = find_sym(&symbols, "hello");
        assert_eq!(hello.kind, "function");
        // Tokens contain identifiers from function body (format, name)
        // Token may be None if all identifiers are filtered as stopwords
        assert_eq!(hello.visibility.as_deref(), Some("public"));

        let helper = find_sym(&symbols, "private_helper");
        assert_eq!(helper.kind, "function");
        assert_eq!(helper.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_rust_struct() {
        let source = b"pub struct Point {
    pub x: i32,
    pub y: i32,
}

struct Private;";
        let (symbols, _texts, _refs) = parse_file(source, "rust", "test.rs").unwrap();
        assert_eq!(symbols.len(), 2);

        let point = find_sym(&symbols, "Point");
        assert_eq!(point.kind, "struct");
        assert_eq!(point.visibility.as_deref(), Some("public"));

        let priv_struct = find_sym(&symbols, "Private");
        assert_eq!(priv_struct.kind, "struct");
        assert_eq!(priv_struct.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_rust_impl() {
        let source = b"struct Foo;

impl Foo {
    pub fn new() -> Self {
        Foo
    }

    fn private_method(&self) {}
}";
        let (symbols, _texts, _refs) = parse_file(source, "rust", "test.rs").unwrap();
        assert_eq!(symbols.len(), 4); // struct + impl + 2 methods

        let _impl_sym = find_sym(&symbols, "Foo");
        // First is struct, second is impl
        let _impl_entry = symbols.iter().find(|s| s.kind == "impl").unwrap();
        // Impl tokens now contain the signature "impl Foo"

        let new = find_sym(&symbols, "Foo.new");
        assert_eq!(new.kind, "method");
        assert_eq!(new.parent.as_deref(), Some("Foo"));
        assert_eq!(new.visibility.as_deref(), Some("public"));

        let priv_method = find_sym(&symbols, "Foo.private_method");
        assert_eq!(priv_method.kind, "method");
        assert_eq!(priv_method.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_rust_trait() {
        let source = b"pub trait Display {
    fn fmt(&self) -> String;
}

impl Display for Foo {
    fn fmt(&self) -> String {
        String::new()
    }
}";
        let (symbols, _texts, _refs) = parse_file(source, "rust", "test.rs").unwrap();

        let trait_sym = symbols
            .iter()
            .find(|s| s.name == "Display" && s.kind == "interface")
            .unwrap();
        assert_eq!(trait_sym.visibility.as_deref(), Some("public"));

        let trait_impl = symbols.iter().find(|s| s.kind == "trait_impl").unwrap();
        // Trait impls are containers, no tokens
        assert!(trait_impl.tokens.is_none());
    }

    #[test]
    fn test_rust_use() {
        let source = b"use std::collections::HashMap;
use std::io::{self, Read};
pub use std::fmt::Debug;";
        let (symbols, _texts, _refs) = parse_file(source, "rust", "test.rs").unwrap();

        let hashmap = symbols
            .iter()
            .find(|s| s.name == "std::collections::HashMap")
            .unwrap();
        assert_eq!(hashmap.kind, "import");
        assert_eq!(hashmap.visibility.as_deref(), Some("private"));

        let debug = symbols.iter().find(|s| s.name.contains("Debug")).unwrap();
        assert_eq!(debug.kind, "import");
        assert_eq!(debug.visibility.as_deref(), Some("public"));
    }

    #[test]
    fn test_rust_enum() {
        let source = b"pub enum Result<T, E> {
    Ok(T),
    Err(E),
}";
        let (symbols, _texts, _refs) = parse_file(source, "rust", "test.rs").unwrap();
        let result = find_sym(&symbols, "Result");
        assert_eq!(result.kind, "enum");
        assert_eq!(result.visibility.as_deref(), Some("public"));
    }

    #[test]
    fn test_rust_mod() {
        let source = b"pub mod utils;
mod private_mod;";
        let (symbols, _texts, _refs) = parse_file(source, "rust", "test.rs").unwrap();

        let utils = find_sym(&symbols, "utils");
        assert_eq!(utils.kind, "module");
        assert_eq!(utils.visibility.as_deref(), Some("public"));

        let priv_mod = find_sym(&symbols, "private_mod");
        assert_eq!(priv_mod.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_rust_const() {
        let source = b"pub const MAX: usize = 100;
static GLOBAL: i32 = 0;";
        let (symbols, _texts, _refs) = parse_file(source, "rust", "test.rs").unwrap();

        let max = find_sym(&symbols, "MAX");
        assert_eq!(max.kind, "constant");
        assert_eq!(max.visibility.as_deref(), Some("public"));

        let global = find_sym(&symbols, "GLOBAL");
        assert_eq!(global.kind, "constant");
        assert_eq!(global.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_rust_comments() {
        let source = b"/// This is a doc comment
/// for the function
pub fn documented() {}

// Regular comment
fn helper() {}";
        let (_symbols, texts, _refs) = parse_file(source, "rust", "test.rs").unwrap();
        assert!(texts.iter().any(|t| t.kind == "comment"));
    }

    #[test]
    fn test_rust_call_references() {
        let source = b"fn caller() {
    some_function();
    module::nested_call();
    obj.method_call();
}

fn some_function() {}";
        let (_symbols, _texts, refs) = parse_file(source, "rust", "test.rs").unwrap();

        let call_refs: Vec<_> = refs.iter().filter(|r| r.kind == "call").collect();
        assert!(
            call_refs.iter().any(|r| r.name == "some_function"),
            "should find some_function call"
        );
        assert!(
            call_refs
                .iter()
                .any(|r| r.name.contains("module::nested_call")),
            "should find nested call"
        );
    }

    #[test]
    fn test_rust_import_references() {
        let source = b"use std::collections::HashMap;
use std::io::{Read, Write};";
        let (_symbols, _texts, refs) = parse_file(source, "rust", "test.rs").unwrap();

        let import_refs: Vec<_> = refs.iter().filter(|r| r.kind == "import").collect();
        assert!(
            import_refs
                .iter()
                .any(|r| r.name == "std::collections::HashMap"),
            "should find HashMap import"
        );
        assert!(
            import_refs.iter().any(|r| r.name.contains("Read")),
            "should find Read import"
        );
    }

    #[test]
    fn test_rust_type_references() {
        let source = b"struct MyStruct {
    field: OtherType,
}

fn process(input: CustomType) -> ResultType {
    todo!()
}";
        let (_symbols, _texts, refs) = parse_file(source, "rust", "test.rs").unwrap();

        let type_refs: Vec<_> = refs
            .iter()
            .filter(|r| r.kind == "type_annotation")
            .collect();
        assert!(
            type_refs.iter().any(|r| r.name == "OtherType"),
            "should find OtherType reference"
        );
        assert!(
            type_refs.iter().any(|r| r.name == "CustomType"),
            "should find CustomType reference"
        );
        assert!(
            type_refs.iter().any(|r| r.name == "ResultType"),
            "should find ResultType reference"
        );
    }
}
