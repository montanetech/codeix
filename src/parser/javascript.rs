//! JavaScript symbol and text extraction.

use tree_sitter::{Node, Tree};

use crate::index::format::{SymbolEntry, TextEntry};
use crate::parser::helpers::*;
use crate::parser::treesitter::MAX_DEPTH;

pub fn extract(
    tree: &Tree,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
) {
    let root = tree.root_node();
    walk_node(root, source, file_path, None, symbols, texts, 0);
}

fn walk_node(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
    depth: usize,
) {
    // Prevent stack overflow on deeply nested code
    if depth > MAX_DEPTH {
        return;
    }

    let kind = node.kind();

    match kind {
        "function_declaration" => {
            extract_function_decl(node, source, file_path, parent_ctx, symbols);
        }
        "generator_function_declaration" => {
            extract_function_decl(node, source, file_path, parent_ctx, symbols);
        }
        "class_declaration" => {
            extract_class(node, source, file_path, parent_ctx, symbols, texts, depth);
            return; // handled recursively
        }
        "method_definition" => {
            extract_method(node, source, file_path, parent_ctx, symbols);
        }
        "lexical_declaration" | "variable_declaration" => {
            extract_variable_decl(node, source, file_path, parent_ctx, symbols);
        }
        "export_statement" => {
            // Recurse into the exported declaration
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_node(
                    child,
                    source,
                    file_path,
                    parent_ctx,
                    symbols,
                    texts,
                    depth + 1,
                );
            }
            return;
        }
        "import_statement" => {
            extract_import(node, source, file_path, symbols);
        }
        "comment" => {
            extract_js_comment(node, source, file_path, parent_ctx, texts);
            return;
        }
        "string" | "template_string" => {
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
            depth + 1,
        );
    }
}

fn extract_function_decl(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let line = node_line_range(node);
    let _sig = build_function_signature(node, source, &name);

    let is_exported = node
        .parent()
        .map(|p| p.kind() == "export_statement")
        .unwrap_or(false);
    let visibility = if is_exported {
        "public".to_string()
    } else {
        "private".to_string()
    };

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
        full_name,
        kind,
        line,
        parent_ctx,
        None, // TODO: add token extraction
        None,
        Some(visibility),
    );
}

fn extract_class(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
    depth: usize,
) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let line = node_line_range(node);

    let is_exported = node
        .parent()
        .map(|p| p.kind() == "export_statement")
        .unwrap_or(false);
    let visibility = if is_exported {
        "public".to_string()
    } else {
        "private".to_string()
    };

    // Build class signature with extends
    let _sig = build_class_signature(node, source, &name);

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
    };

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        "class",
        line,
        parent_ctx,
        None, // TODO: add token extraction
        None,
        Some(visibility),
    );

    // Walk class body
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
                depth + 1,
            );
        }
    }
}

fn extract_method(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let line = node_line_range(node);

    // Check for static/get/set/async modifiers
    let mut is_static = false;
    let mut is_getter = false;
    let mut is_setter = false;
    let mut is_async = false;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "static" => is_static = true,
            "get" => is_getter = true,
            "set" => is_setter = true,
            "async" => is_async = true,
            _ => {}
        }
    }

    let kind = if is_getter || is_setter {
        "property"
    } else {
        "method"
    };

    let params = find_child_by_field(node, "parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_else(|| "()".to_string());

    let mut sig_parts = Vec::new();
    if is_async {
        sig_parts.push("async");
    }
    if is_static {
        sig_parts.push("static");
    }
    if is_getter {
        sig_parts.push("get");
    }
    if is_setter {
        sig_parts.push("set");
    }
    let prefix = if sig_parts.is_empty() {
        String::new()
    } else {
        format!("{} ", sig_parts.join(" "))
    };
    let _sig = format!("{prefix}{name}{params}");

    let visibility = if name.starts_with('#') {
        "private".to_string()
    } else if name.starts_with('_') {
        "internal".to_string()
    } else {
        "public".to_string()
    };

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name
    };

    push_symbol(
        symbols,
        file_path,
        full_name,
        kind,
        line,
        parent_ctx,
        None, // TODO: add token extraction
        None,
        Some(visibility),
    );
}

fn extract_variable_decl(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
) {
    let line = node_line_range(node);

    let is_exported = node
        .parent()
        .map(|p| p.kind() == "export_statement")
        .unwrap_or(false);
    let visibility = if is_exported {
        "public".to_string()
    } else {
        "private".to_string()
    };

    // Determine if const
    let is_const = node.kind() == "lexical_declaration" && {
        node.child(0)
            .map(|c| node_text(c, source) == "const")
            .unwrap_or(false)
    };

    // Walk declarators
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            let name_node = find_child_by_field(child, "name");
            let value_node = find_child_by_field(child, "value");

            if let Some(n) = name_node {
                // Only index simple identifiers, not destructuring patterns
                if n.kind() != "identifier" {
                    continue;
                }
                let name = node_text(n, source);

                // Check if the value is a function/arrow function
                let is_func = value_node
                    .map(|v| {
                        matches!(
                            v.kind(),
                            "arrow_function"
                                | "function"
                                | "function_expression"
                                | "generator_function"
                        )
                    })
                    .unwrap_or(false);

                let kind = if is_func {
                    "function"
                } else if is_const
                    && name.chars().all(|c| c.is_uppercase() || c == '_')
                    && name.len() > 1
                {
                    "constant"
                } else {
                    "variable"
                };

                let full_name = if let Some(parent) = parent_ctx {
                    format!("{parent}.{name}")
                } else {
                    name
                };

                push_symbol(
                    symbols,
                    file_path,
                    full_name,
                    kind,
                    line,
                    parent_ctx,
                    None, // TODO: add token extraction
                    None,
                    Some(visibility.clone()),
                );
            }
        }
    }
}

fn extract_import(node: Node, source: &[u8], file_path: &str, symbols: &mut Vec<SymbolEntry>) {
    let line = node_line_range(node);

    // Get the source module
    let source_module = find_child_by_field(node, "source")
        .map(|n| {
            let raw = node_text(n, source);
            strip_string_quotes(&raw)
        })
        .unwrap_or_default();

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "import_clause" {
            let mut clause_cursor = child.walk();
            for clause_child in child.children(&mut clause_cursor) {
                match clause_child.kind() {
                    "identifier" => {
                        // Default import: `import foo from "..."`
                        let name = node_text(clause_child, source);
                        let full_name = source_module.clone();
                        push_symbol(
                            symbols,
                            file_path,
                            full_name,
                            "import",
                            line,
                            None,
                            None,
                            Some(name),
                            Some("private".to_string()),
                        );
                    }
                    "named_imports" => {
                        // `import { foo, bar as baz } from "..."`
                        let mut named_cursor = clause_child.walk();
                        for spec in clause_child.children(&mut named_cursor) {
                            if spec.kind() == "import_specifier" {
                                let imported_name =
                                    find_child_by_field(spec, "name").map(|n| node_text(n, source));
                                let alias = find_child_by_field(spec, "alias")
                                    .map(|n| node_text(n, source));

                                if let Some(imp_name) = imported_name {
                                    let full_name = format!("{source_module}.{imp_name}");
                                    push_symbol(
                                        symbols,
                                        file_path,
                                        full_name,
                                        "import",
                                        line,
                                        None,
                                        None,
                                        alias,
                                        Some("private".to_string()),
                                    );
                                }
                            }
                        }
                    }
                    "namespace_import" => {
                        // `import * as foo from "..."`
                        let alias = find_child_by_field(clause_child, "alias")
                            .or_else(|| {
                                // In some grammars, the identifier is a direct child
                                let mut c = clause_child.walk();
                                clause_child
                                    .children(&mut c)
                                    .find(|n| n.kind() == "identifier")
                            })
                            .map(|n| node_text(n, source));
                        let full_name = format!("{source_module}.*");
                        push_symbol(
                            symbols,
                            file_path,
                            full_name,
                            "import",
                            line,
                            None,
                            None,
                            alias,
                            Some("private".to_string()),
                        );
                    }
                    _ => {}
                }
            }
        }
    }
}

fn extract_js_comment(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    let raw = node_text(node, source);
    let line = node_line_range(node);

    let (kind, text) = if raw.starts_with("/**") {
        // JSDoc comment
        let cleaned = strip_block_comment(&raw);
        ("docstring", cleaned)
    } else if raw.starts_with("/*") {
        let cleaned = strip_block_comment(&raw);
        ("comment", cleaned)
    } else if raw.starts_with("//") {
        let cleaned = raw.strip_prefix("//").unwrap_or(&raw).trim().to_string();
        ("comment", cleaned)
    } else {
        ("comment", raw)
    };

    if is_trivial_text(&text) {
        return;
    }

    texts.push(TextEntry {
        file: file_path.to_string(),
        kind: kind.to_string(),
        line,
        text,
        parent: parent_ctx.map(String::from),
        project: String::new(),
    });
}

fn build_function_signature(node: Node, source: &[u8], name: &str) -> String {
    let params = find_child_by_field(node, "parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_else(|| "()".to_string());

    let is_async = node.child(0).map(|c| c.kind() == "async").unwrap_or(false);

    let is_generator = node.kind() == "generator_function_declaration";

    let prefix = match (is_async, is_generator) {
        (true, true) => "async function*",
        (true, false) => "async function",
        (false, true) => "function*",
        (false, false) => "function",
    };

    format!("{prefix} {name}{params}")
}

fn build_class_signature(node: Node, source: &[u8], name: &str) -> String {
    // Check for extends clause
    let extends = find_child_by_field(node, "heritage")
        .or_else(|| {
            let mut cursor = node.walk();
            node.children(&mut cursor)
                .find(|c| c.kind() == "class_heritage")
        })
        .map(|n| {
            let text = node_text(n, source);
            format!(" {text}")
        })
        .unwrap_or_default();

    format!("class {name}{extends}")
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
    fn test_js_functions() {
        let source = b"function hello(name) {
    return `Hello, ${name}!`;
}

async function fetchData() {
    return await fetch('/api');
}

function* generator() {
    yield 1;
}";
        let (symbols, _texts) = parse_file(source, "javascript", "test.js").unwrap();
        assert_eq!(symbols.len(), 3);

        let hello = find_sym(&symbols, "hello");
        assert_eq!(hello.kind, "function");
        // Token extraction not yet implemented for JavaScript
        assert!(hello.tokens.is_none());
        assert_eq!(hello.visibility.as_deref(), Some("private"));

        let fetch = find_sym(&symbols, "fetchData");
        assert_eq!(fetch.kind, "function");

        let generator = find_sym(&symbols, "generator");
        assert_eq!(generator.kind, "function");
    }

    #[test]
    fn test_js_classes() {
        let source = b"export class Person {
    constructor(name) {
        this.name = name;
    }

    greet() {
        return `Hi, ${this.name}`;
    }

    static create() {
        return new Person('default');
    }

    get fullName() {
        return this.name;
    }
}";
        let (symbols, _texts) = parse_file(source, "javascript", "test.js").unwrap();

        let person = find_sym(&symbols, "Person");
        assert_eq!(person.kind, "class");
        assert_eq!(person.visibility.as_deref(), Some("public"));

        let greet = find_sym(&symbols, "Person.greet");
        assert_eq!(greet.kind, "method");
        assert_eq!(greet.parent.as_deref(), Some("Person"));

        let create = find_sym(&symbols, "Person.create");
        assert_eq!(create.kind, "method");

        let getter = find_sym(&symbols, "Person.fullName");
        assert_eq!(getter.kind, "property");
    }

    #[test]
    fn test_js_variables() {
        let source = b"const MAX_SIZE = 100;
let debug = true;
var legacy = 'old';

export const API_KEY = 'secret';

const add = (a, b) => a + b;
const asyncFn = async (x) => x * 2;";
        let (symbols, _texts) = parse_file(source, "javascript", "test.js").unwrap();

        let max = find_sym(&symbols, "MAX_SIZE");
        assert_eq!(max.kind, "constant");

        let debug = find_sym(&symbols, "debug");
        assert_eq!(debug.kind, "variable");

        let api = find_sym(&symbols, "API_KEY");
        assert_eq!(api.visibility.as_deref(), Some("public"));

        let add = find_sym(&symbols, "add");
        assert_eq!(add.kind, "function");

        let async_fn = find_sym(&symbols, "asyncFn");
        assert_eq!(async_fn.kind, "function");
    }

    #[test]
    fn test_js_imports() {
        let source = b"import React from 'react';
import { useState, useEffect } from 'react';
import * as Utils from './utils';
import { render as renderDOM } from 'react-dom';";
        let (symbols, _texts) = parse_file(source, "javascript", "test.js").unwrap();

        let react = symbols.iter().find(|s| s.name == "react").unwrap();
        assert_eq!(react.kind, "import");
        assert_eq!(react.alias.as_deref(), Some("React"));

        let use_state = symbols.iter().find(|s| s.name == "react.useState").unwrap();
        assert_eq!(use_state.kind, "import");

        let utils = symbols.iter().find(|s| s.name == "./utils.*").unwrap();
        assert_eq!(utils.alias.as_deref(), Some("Utils"));

        let render = symbols
            .iter()
            .find(|s| s.name == "react-dom.render")
            .unwrap();
        assert_eq!(render.alias.as_deref(), Some("renderDOM"));
    }

    #[test]
    fn test_js_visibility() {
        let source = b"class Foo {
    publicMethod() {}

    _internalMethod() {}

    #privateMethod() {}
}";
        let (symbols, _texts) = parse_file(source, "javascript", "test.js").unwrap();

        let public = symbols
            .iter()
            .find(|s| s.name == "Foo.publicMethod")
            .unwrap();
        assert_eq!(public.visibility.as_deref(), Some("public"));

        let internal = symbols
            .iter()
            .find(|s| s.name == "Foo._internalMethod")
            .unwrap();
        assert_eq!(internal.visibility.as_deref(), Some("internal"));

        let private = symbols
            .iter()
            .find(|s| s.name == "Foo.#privateMethod")
            .unwrap();
        assert_eq!(private.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_js_comments() {
        let source = b"/**
 * JSDoc comment
 */
function documented() {}

// Single line comment
function helper() {}

/* Block comment */";
        let (_symbols, texts) = parse_file(source, "javascript", "test.js").unwrap();
        assert!(texts.iter().any(|t| t.kind == "docstring"));
        assert!(texts.iter().any(|t| t.kind == "comment"));
    }
}
