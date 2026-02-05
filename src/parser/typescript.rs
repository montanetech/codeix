//! TypeScript symbol and text extraction.
//!
//! TypeScript extends JavaScript with type annotations, interfaces,
//! enums, type aliases, and namespaces. We reuse the JS extraction
//! for most constructs and add TS-specific ones.

use tree_sitter::{Node, Tree};

use crate::index::format::{SymbolEntry, TextEntry};
use crate::parser::helpers::*;

pub fn extract(
    tree: &Tree,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
) {
    let root = tree.root_node();
    walk_node(root, source, file_path, None, symbols, texts);
}

fn walk_node(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
) {
    let kind = node.kind();

    match kind {
        // --- JS constructs ---
        "function_declaration" | "generator_function_declaration" => {
            extract_function_decl(node, source, file_path, parent_ctx, symbols);
        }
        "class_declaration" => {
            extract_class(node, source, file_path, parent_ctx, symbols, texts);
            return;
        }
        "method_definition" => {
            extract_method(node, source, file_path, parent_ctx, symbols);
        }
        "lexical_declaration" | "variable_declaration" => {
            extract_variable_decl(node, source, file_path, parent_ctx, symbols);
        }
        "export_statement" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_node(child, source, file_path, parent_ctx, symbols, texts);
            }
            return;
        }
        "import_statement" => {
            extract_import(node, source, file_path, symbols);
        }

        // --- TS-specific constructs ---
        "interface_declaration" => {
            extract_interface(node, source, file_path, parent_ctx, symbols, texts);
            return;
        }
        "type_alias_declaration" => {
            extract_type_alias(node, source, file_path, parent_ctx, symbols);
        }
        "enum_declaration" => {
            extract_enum(node, source, file_path, parent_ctx, symbols);
        }
        "module" | "internal_module" => {
            // `namespace Foo { ... }` or `module Foo { ... }`
            extract_namespace(node, source, file_path, parent_ctx, symbols, texts);
            return;
        }
        "abstract_class_declaration" => {
            extract_class(node, source, file_path, parent_ctx, symbols, texts);
            return;
        }

        "comment" => {
            extract_ts_comment(node, source, file_path, parent_ctx, texts);
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
        walk_node(child, source, file_path, parent_ctx, symbols, texts);
    }
}

// --- Shared JS-like extraction (adapted for TS node names) ---

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
    let sig = build_function_signature(node, source, &name);

    let is_exported = node
        .parent()
        .map(|p| p.kind() == "export_statement")
        .unwrap_or(false);
    let visibility = if is_exported { "public" } else { "private" };

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
        Some(sig),
        None,
        Some(visibility.to_string()),
    );
}

fn extract_class(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
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
    let visibility = if is_exported { "public" } else { "private" };

    let is_abstract = node.kind() == "abstract_class_declaration";
    let sig = build_class_signature(node, source, &name, is_abstract);

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
        Some(sig),
        None,
        Some(visibility.to_string()),
    );

    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            walk_node(child, source, file_path, Some(&full_name), symbols, texts);
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

    let mut is_static = false;
    let mut is_getter = false;
    let mut is_setter = false;
    let mut is_async = false;
    let mut access_modifier = None;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "static" => is_static = true,
            "get" => is_getter = true,
            "set" => is_setter = true,
            "async" => is_async = true,
            "accessibility_modifier" => {
                access_modifier = Some(node_text(child, source));
            }
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

    let return_type = find_child_by_field(node, "return_type")
        .map(|n| format!(": {}", node_text(n, source)))
        .unwrap_or_default();

    let mut sig_parts = Vec::new();
    if is_async {
        sig_parts.push("async".to_string());
    }
    if is_static {
        sig_parts.push("static".to_string());
    }
    if is_getter {
        sig_parts.push("get".to_string());
    }
    if is_setter {
        sig_parts.push("set".to_string());
    }
    let prefix = if sig_parts.is_empty() {
        String::new()
    } else {
        format!("{} ", sig_parts.join(" "))
    };
    let sig = format!("{prefix}{name}{params}{return_type}");

    let visibility = match access_modifier.as_deref() {
        Some("private") => "private",
        Some("protected") => "internal",
        _ if name.starts_with('#') => "private",
        _ => "public",
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
        Some(sig),
        None,
        Some(visibility.to_string()),
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
    let visibility = if is_exported { "public" } else { "private" };

    let is_const = node.kind() == "lexical_declaration" && {
        node.child(0)
            .map(|c| node_text(c, source) == "const")
            .unwrap_or(false)
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            let name_node = find_child_by_field(child, "name");
            let value_node = find_child_by_field(child, "value");

            if let Some(n) = name_node {
                if n.kind() != "identifier" {
                    continue;
                }
                let name = node_text(n, source);

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

                let (kind, sig) = if is_func {
                    let sig = build_arrow_fn_signature(&name, value_node.unwrap(), source);
                    ("function", Some(sig))
                } else if is_const
                    && name.chars().all(|c| c.is_uppercase() || c == '_')
                    && name.len() > 1
                {
                    ("constant", None)
                } else {
                    ("variable", None)
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
                    sig,
                    None,
                    Some(visibility.to_string()),
                );
            }
        }
    }
}

fn extract_import(node: Node, source: &[u8], file_path: &str, symbols: &mut Vec<SymbolEntry>) {
    let line = node_line_range(node);

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
                        let name = node_text(clause_child, source);
                        push_symbol(
                            symbols,
                            file_path,
                            source_module.clone(),
                            "import",
                            line,
                            None,
                            None,
                            Some(name),
                            Some("private".to_string()),
                        );
                    }
                    "named_imports" => {
                        let mut named_cursor = clause_child.walk();
                        for spec in clause_child.children(&mut named_cursor) {
                            if spec.kind() == "import_specifier" {
                                let imp_name =
                                    find_child_by_field(spec, "name").map(|n| node_text(n, source));
                                let alias = find_child_by_field(spec, "alias")
                                    .map(|n| node_text(n, source));
                                if let Some(name) = imp_name {
                                    let full = format!("{source_module}.{name}");
                                    push_symbol(
                                        symbols,
                                        file_path,
                                        full,
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
                        let alias = find_child_by_field(clause_child, "alias")
                            .or_else(|| {
                                let mut c = clause_child.walk();
                                clause_child
                                    .children(&mut c)
                                    .find(|n| n.kind() == "identifier")
                            })
                            .map(|n| node_text(n, source));
                        let full = format!("{source_module}.*");
                        push_symbol(
                            symbols,
                            file_path,
                            full,
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

// --- TS-specific constructs ---

fn extract_interface(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
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
    let visibility = if is_exported { "public" } else { "private" };

    // Build signature with type parameters and extends
    let type_params = find_child_by_field(node, "type_parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    let extends = find_child_by_field(node, "extends_type")
        .or_else(|| {
            let mut cursor = node.walk();
            node.children(&mut cursor)
                .find(|c| c.kind() == "extends_type_clause")
        })
        .map(|n| format!(" extends {}", node_text(n, source)))
        .unwrap_or_default();

    let sig = format!("interface {name}{type_params}{extends}");

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
    };

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        "interface",
        line,
        parent_ctx,
        Some(sig),
        None,
        Some(visibility.to_string()),
    );

    // Walk interface body for method signatures
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            match child.kind() {
                "method_signature" | "property_signature" => {
                    if let Some(n) = find_child_by_field(child, "name") {
                        let member_name = node_text(n, source);
                        let member_line = node_line_range(child);
                        let member_kind = if child.kind() == "method_signature" {
                            "method"
                        } else {
                            "property"
                        };
                        let member_sig = collapse_whitespace(node_text(child, source).trim());
                        push_symbol(
                            symbols,
                            file_path,
                            format!("{full_name}.{member_name}"),
                            member_kind,
                            member_line,
                            Some(&full_name),
                            Some(member_sig),
                            None,
                            Some("public".to_string()),
                        );
                    }
                }
                "comment" => {
                    extract_ts_comment(child, source, file_path, Some(&full_name), texts);
                }
                _ => {}
            }
        }
    }
}

fn extract_type_alias(
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
    let is_exported = node
        .parent()
        .map(|p| p.kind() == "export_statement")
        .unwrap_or(false);
    let visibility = if is_exported { "public" } else { "private" };

    let type_params = find_child_by_field(node, "type_parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    let sig = format!("type {name}{type_params}");

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name
    };

    push_symbol(
        symbols,
        file_path,
        full_name,
        "type_alias",
        line,
        parent_ctx,
        Some(sig),
        None,
        Some(visibility.to_string()),
    );
}

fn extract_enum(
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
    let is_exported = node
        .parent()
        .map(|p| p.kind() == "export_statement")
        .unwrap_or(false);
    let visibility = if is_exported { "public" } else { "private" };

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name
    };

    push_symbol(
        symbols,
        file_path,
        full_name,
        "enum",
        line,
        parent_ctx,
        None,
        None,
        Some(visibility.to_string()),
    );
}

fn extract_namespace(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
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
    let visibility = if is_exported { "public" } else { "private" };

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
    };

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        "module",
        line,
        parent_ctx,
        None,
        None,
        Some(visibility.to_string()),
    );

    // Recurse into namespace body
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            walk_node(child, source, file_path, Some(&full_name), symbols, texts);
        }
    }
}

// --- Helpers ---

fn extract_ts_comment(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    let raw = node_text(node, source);
    let line = node_line_range(node);

    let (kind, text) = if raw.starts_with("/**") {
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
    });
}

fn build_function_signature(node: Node, source: &[u8], name: &str) -> String {
    let params = find_child_by_field(node, "parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_else(|| "()".to_string());

    let return_type = find_child_by_field(node, "return_type")
        .map(|n| format!(": {}", node_text(n, source)))
        .unwrap_or_default();

    let type_params = find_child_by_field(node, "type_parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    let is_async = node.child(0).map(|c| c.kind() == "async").unwrap_or(false);

    let prefix = if is_async {
        "async function"
    } else {
        "function"
    };

    format!("{prefix} {name}{type_params}{params}{return_type}")
}

fn build_arrow_fn_signature(name: &str, value_node: Node, source: &[u8]) -> String {
    let params = find_child_by_field(value_node, "parameters")
        .or_else(|| find_child_by_field(value_node, "parameter"))
        .map(|n| {
            let text = node_text(n, source);
            if n.kind() == "identifier" {
                format!("({text})")
            } else {
                text
            }
        })
        .unwrap_or_else(|| "()".to_string());

    let return_type = find_child_by_field(value_node, "return_type")
        .map(|n| format!(": {}", node_text(n, source)))
        .unwrap_or_default();

    let is_async = value_node
        .child(0)
        .map(|c| c.kind() == "async")
        .unwrap_or(false);

    if is_async {
        format!("async {name}{params}{return_type}")
    } else {
        format!("{name}{params}{return_type}")
    }
}

fn build_class_signature(node: Node, source: &[u8], name: &str, is_abstract: bool) -> String {
    let type_params = find_child_by_field(node, "type_parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    let extends = find_child_by_field(node, "heritage")
        .or_else(|| {
            let mut cursor = node.walk();
            node.children(&mut cursor)
                .find(|c| c.kind() == "class_heritage")
        })
        .map(|n| format!(" {}", node_text(n, source)))
        .unwrap_or_default();

    let prefix = if is_abstract {
        "abstract class"
    } else {
        "class"
    };

    format!("{prefix} {name}{type_params}{extends}")
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
    fn test_ts_functions() {
        let source = b"function greet(name: string): string {
    return `Hello, ${name}!`;
}

async function fetch(): Promise<Data> {
    return await api.get();
}";
        let (symbols, _texts) = parse_file(source, "typescript", "test.ts").unwrap();
        assert_eq!(symbols.len(), 2);

        let greet = find_sym(&symbols, "greet");
        assert_eq!(greet.kind, "function");
        assert!(greet.sig.as_ref().unwrap().contains("string"));
        assert!(greet.sig.as_ref().unwrap().contains(": string"));

        let fetch_fn = find_sym(&symbols, "fetch");
        assert!(fetch_fn.sig.as_ref().unwrap().contains("async"));
        assert!(fetch_fn.sig.as_ref().unwrap().contains("Promise"));
    }

    #[test]
    fn test_ts_interfaces() {
        let source = b"export interface User {
    id: number;
    name: string;
    getEmail(): string;
}

interface Private {
    data: any;
}";
        let (symbols, _texts) = parse_file(source, "typescript", "test.ts").unwrap();

        let user = find_sym(&symbols, "User");
        assert_eq!(user.kind, "interface");
        assert_eq!(user.visibility.as_deref(), Some("public"));
        assert!(user.sig.as_ref().unwrap().contains("interface User"));

        let get_email = find_sym(&symbols, "User.getEmail");
        assert_eq!(get_email.kind, "method");
        assert_eq!(get_email.parent.as_deref(), Some("User"));

        let priv_iface = find_sym(&symbols, "Private");
        assert_eq!(priv_iface.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_ts_type_alias() {
        let source = b"export type Result<T> = Success<T> | Error;
type ID = string | number;";
        let (symbols, _texts) = parse_file(source, "typescript", "test.ts").unwrap();

        let result = find_sym(&symbols, "Result");
        assert_eq!(result.kind, "type_alias");
        assert!(result.sig.as_ref().unwrap().contains("type Result"));
        assert_eq!(result.visibility.as_deref(), Some("public"));

        let id = find_sym(&symbols, "ID");
        assert_eq!(id.kind, "type_alias");
        assert_eq!(id.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_ts_enum() {
        let source = b"export enum Status {
    Active,
    Inactive,
    Pending
}";
        let (symbols, _texts) = parse_file(source, "typescript", "test.ts").unwrap();

        let status = find_sym(&symbols, "Status");
        assert_eq!(status.kind, "enum");
        assert_eq!(status.visibility.as_deref(), Some("public"));
    }

    #[test]
    fn test_ts_classes() {
        let source = b"export abstract class Base {
    protected abstract doWork(): void;
}

export class Worker extends Base {
    private data: string;

    constructor(data: string) {
        super();
        this.data = data;
    }

    protected doWork(): void {
        console.log(this.data);
    }

    public run(): void {
        this.doWork();
    }
}";
        let (symbols, _texts) = parse_file(source, "typescript", "test.ts").unwrap();

        let base = find_sym(&symbols, "Base");
        assert_eq!(base.kind, "class");
        assert!(base.sig.as_ref().unwrap().contains("abstract class"));

        let worker = find_sym(&symbols, "Worker");
        assert!(worker.sig.as_ref().unwrap().contains("extends"));

        let do_work = symbols.iter().find(|s| s.name == "Worker.doWork").unwrap();
        assert_eq!(do_work.visibility.as_deref(), Some("internal"));

        let run = symbols.iter().find(|s| s.name == "Worker.run").unwrap();
        assert_eq!(run.visibility.as_deref(), Some("public"));
    }

    #[test]
    fn test_ts_namespace() {
        let source = b"export namespace Utils {
    export function helper(): void {}
}";
        let (symbols, _texts) = parse_file(source, "typescript", "test.ts").unwrap();

        let utils = find_sym(&symbols, "Utils");
        assert_eq!(utils.kind, "module");
        assert_eq!(utils.visibility.as_deref(), Some("public"));

        let helper = find_sym(&symbols, "Utils.helper");
        assert_eq!(helper.parent.as_deref(), Some("Utils"));
    }

    #[test]
    fn test_ts_imports() {
        let source = b"import React from 'react';
import { Component, useState } from 'react';
import * as Utils from './utils';
import type { User } from './types';";
        let (symbols, _texts) = parse_file(source, "typescript", "test.ts").unwrap();

        let react = symbols.iter().find(|s| s.name == "react").unwrap();
        assert_eq!(react.alias.as_deref(), Some("React"));

        let component = symbols
            .iter()
            .find(|s| s.name == "react.Component")
            .unwrap();
        assert_eq!(component.kind, "import");

        let utils = symbols.iter().find(|s| s.name == "./utils.*").unwrap();
        assert_eq!(utils.alias.as_deref(), Some("Utils"));
    }

    #[test]
    fn test_ts_visibility() {
        let source = b"class Foo {
    public publicMethod(): void {}
    private privateMethod(): void {}
    protected protectedMethod(): void {}
}";
        let (symbols, _texts) = parse_file(source, "typescript", "test.ts").unwrap();

        let public = symbols
            .iter()
            .find(|s| s.name == "Foo.publicMethod")
            .unwrap();
        assert_eq!(public.visibility.as_deref(), Some("public"));

        let private = symbols
            .iter()
            .find(|s| s.name == "Foo.privateMethod")
            .unwrap();
        assert_eq!(private.visibility.as_deref(), Some("private"));

        let protected = symbols
            .iter()
            .find(|s| s.name == "Foo.protectedMethod")
            .unwrap();
        assert_eq!(protected.visibility.as_deref(), Some("internal"));
    }
}
