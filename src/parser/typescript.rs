//! TypeScript symbol and text extraction.
//!
//! TypeScript extends JavaScript with type annotations, interfaces,
//! enums, type aliases, and namespaces. We reuse the JS extraction
//! for most constructs and add TS-specific ones.

use tree_sitter::{Node, Tree};

use crate::index::format::{ReferenceEntry, SymbolEntry, TextEntry};
use crate::parser::helpers::*;
use crate::parser::treesitter::MAX_DEPTH;

/// TypeScript-specific stopwords (JS keywords + TS type system keywords)
const TS_STOPWORDS: &[&str] = &[
    // JavaScript common
    "undefined",
    "null",
    "console",
    "window",
    "document",
    "exports",
    "module",
    "require",
    "import",
    "export",
    "from",
    "let",
    "var",
    "function",
    "extends",
    "finally",
    "async",
    "await",
    "yield",
    "typeof",
    "instanceof",
    "delete",
    "of",
    "prototype",
    "constructor",
    "length",
    "name",
    "arguments",
    // TypeScript-specific
    "type",
    "interface",
    "namespace",
    "declare",
    "readonly",
    "abstract",
    "override",
    "implements",
    "keyof",
    "infer",
    "never",
    "unknown",
    "any",
    "object",
    "string",
    "number",
    "boolean",
    "symbol",
    "bigint",
    "Promise",
    "Array",
    "Object",
    "String",
    "Number",
    "Boolean",
];

/// Filter TypeScript-specific stopwords from extracted tokens.
fn filter_ts_tokens(tokens: Option<String>) -> Option<String> {
    tokens.and_then(|t| {
        let filtered: Vec<&str> = t
            .split_whitespace()
            .filter(|tok| !TS_STOPWORDS.contains(&tok.to_lowercase().as_str()))
            .collect();
        if filtered.is_empty() {
            None
        } else {
            Some(filtered.join(" "))
        }
    })
}

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

// ---------------------------------------------------------------------------
// Builtin detection for filtering noisy references
// ---------------------------------------------------------------------------

/// Check if a function name is a TypeScript/JavaScript builtin.
fn is_ts_builtin_call(name: &str) -> bool {
    // Check for builtin object method calls (e.g., console.log, Math.floor)
    if let Some(obj) = name.split('.').next()
        && matches!(
            obj,
            "console"
                | "Math"
                | "JSON"
                | "Object"
                | "Array"
                | "String"
                | "Number"
                | "Date"
                | "RegExp"
                | "Promise"
                | "Reflect"
                | "Proxy"
        )
    {
        return true;
    }

    matches!(
        name,
        // Console methods (note: "log" also used by Math.log so listed once under Math)
        "console"
        | "error"
        | "warn"
        | "info"
        | "debug"
        | "trace"
        | "dir"
        | "table"
        | "time"
        | "timeEnd"
        | "clear"
        | "count"
        | "countReset"
        | "group"
        | "groupEnd"
        | "assert"
        // Global functions
        | "parseInt"
        | "parseFloat"
        | "isNaN"
        | "isFinite"
        | "encodeURI"
        | "decodeURI"
        | "encodeURIComponent"
        | "decodeURIComponent"
        | "eval"
        | "setTimeout"
        | "setInterval"
        | "clearTimeout"
        | "clearInterval"
        | "fetch"
        | "require"
        // Object/Array methods
        | "Object"
        | "Array"
        | "String"
        | "Number"
        | "Boolean"
        | "Symbol"
        | "BigInt"
        | "Date"
        | "RegExp"
        | "Error"
        | "Map"
        | "Set"
        | "WeakMap"
        | "WeakSet"
        | "Promise"
        | "Proxy"
        | "Reflect"
        | "JSON"
        | "Math"
        // Common array/object methods
        | "push"
        | "pop"
        | "shift"
        | "unshift"
        | "slice"
        | "splice"
        | "concat"
        | "join"
        | "reverse"
        | "sort"
        | "filter"
        | "map"
        | "reduce"
        | "reduceRight"
        | "forEach"
        | "find"
        | "findIndex"
        | "indexOf"
        | "includes"
        | "every"
        | "some"
        | "flat"
        | "flatMap"
        | "keys"
        | "values"
        | "entries"
        | "from"
        | "of"
        | "isArray"
        // String methods
        | "charAt"
        | "charCodeAt"
        | "substring"
        | "substr"
        | "replace"
        | "replaceAll"
        | "split"
        | "toLowerCase"
        | "toUpperCase"
        | "trim"
        | "trimStart"
        | "trimEnd"
        | "padStart"
        | "padEnd"
        | "repeat"
        | "startsWith"
        | "endsWith"
        | "match"
        | "matchAll"
        | "search"
        | "localeCompare"
        // Object methods
        | "hasOwnProperty"
        | "toString"
        | "valueOf"
        | "toLocaleString"
        | "assign"
        | "create"
        | "defineProperty"
        | "defineProperties"
        | "freeze"
        | "seal"
        | "getPrototypeOf"
        | "setPrototypeOf"
        | "getOwnPropertyNames"
        | "getOwnPropertySymbols"
        | "getOwnPropertyDescriptor"
        // Promise methods
        | "then"
        | "catch"
        | "finally"
        | "resolve"
        | "reject"
        | "all"
        | "race"
        | "allSettled"
        | "any"
        // Math methods
        | "abs"
        | "ceil"
        | "floor"
        | "round"
        | "max"
        | "min"
        | "pow"
        | "sqrt"
        | "random"
        | "sin"
        | "cos"
        | "tan"
        | "log"
        | "exp"
        // JSON methods
        | "parse"
        | "stringify"
        // Test assertions (common test frameworks)
        | "describe"
        | "it"
        | "test"
        | "expect"
        | "beforeEach"
        | "afterEach"
        | "beforeAll"
        | "afterAll"
        | "jest"
        | "mock"
        | "spyOn"
        | "toBe"
        | "toEqual"
        | "toContain"
        | "toThrow"
        | "toHaveBeenCalled"
        | "toHaveBeenCalledWith"
    )
}

/// Check if a type name is a TypeScript primitive type.
fn is_ts_primitive_type(name: &str) -> bool {
    matches!(
        name,
        "any"
            | "unknown"
            | "never"
            | "void"
            | "undefined"
            | "null"
            | "string"
            | "number"
            | "boolean"
            | "bigint"
            | "symbol"
            | "object"
            | "Function"
            | "Object"
            | "Array"
            | "String"
            | "Number"
            | "Boolean"
            | "Symbol"
            | "Date"
            | "RegExp"
            | "Error"
            | "Map"
            | "Set"
            | "WeakMap"
            | "WeakSet"
            | "Promise"
            | "Readonly"
            | "Partial"
            | "Required"
            | "Pick"
            | "Omit"
            | "Record"
            | "Exclude"
            | "Extract"
            | "NonNullable"
            | "Parameters"
            | "ReturnType"
            | "InstanceType"
            | "ThisType"
            | "Awaited"
    )
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
        // --- JS constructs ---
        "function_declaration" | "generator_function_declaration" => {
            extract_function_decl(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return; // handled recursively
        }
        "class_declaration" => {
            extract_class(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return;
        }
        "method_definition" => {
            extract_method(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return; // handled recursively
        }
        "lexical_declaration" | "variable_declaration" => {
            extract_variable_decl(node, source, file_path, parent_ctx, symbols);
        }
        "export_statement" => {
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
            return;
        }
        "import_statement" => {
            extract_import(node, source, file_path, symbols, references);
        }

        // --- TS-specific constructs ---
        "interface_declaration" => {
            extract_interface(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return;
        }
        "type_alias_declaration" => {
            extract_type_alias(node, source, file_path, parent_ctx, symbols, references);
        }
        "enum_declaration" => {
            extract_enum(node, source, file_path, parent_ctx, symbols);
        }
        "module" | "internal_module" => {
            // `namespace Foo { ... }` or `module Foo { ... }`
            extract_namespace(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return;
        }
        "abstract_class_declaration" => {
            extract_class(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return;
        }

        // --- Reference extraction ---
        "call_expression" => {
            extract_call_ref(node, source, file_path, parent_ctx, references);
        }
        "new_expression" => {
            extract_new_ref(node, source, file_path, parent_ctx, references);
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

// ---------------------------------------------------------------------------
// Reference extraction
// ---------------------------------------------------------------------------

/// Extract a function call reference.
fn extract_call_ref(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    references: &mut Vec<ReferenceEntry>,
) {
    let func = match find_child_by_field(node, "function") {
        Some(f) => f,
        None => return,
    };

    let name = get_call_name(func, source);
    if name.is_empty() || is_ts_builtin_call(&name) {
        return;
    }

    let line = node_line_range(node);
    references.push(ReferenceEntry {
        file: file_path.to_string(),
        name,
        kind: "call".to_string(),
        line,
        caller: parent_ctx.map(String::from),
        project: String::new(),
    });
}

/// Extract a `new` expression reference (instantiation).
fn extract_new_ref(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    references: &mut Vec<ReferenceEntry>,
) {
    let constructor = match find_child_by_field(node, "constructor") {
        Some(c) => c,
        None => return,
    };

    let name = get_call_name(constructor, source);
    if name.is_empty() || is_ts_builtin_call(&name) || is_ts_primitive_type(&name) {
        return;
    }

    let line = node_line_range(node);
    references.push(ReferenceEntry {
        file: file_path.to_string(),
        name,
        kind: "instantiation".to_string(),
        line,
        caller: parent_ctx.map(String::from),
        project: String::new(),
    });
}

/// Get the name of a function/method call.
fn get_call_name(node: Node, source: &[u8]) -> String {
    match node.kind() {
        "identifier" => node_text(node, source),
        "member_expression" => {
            // obj.method or obj.prop.method
            if let Some(prop) = find_child_by_field(node, "property") {
                if let Some(obj) = find_child_by_field(node, "object") {
                    let obj_name = get_call_name(obj, source);
                    let prop_name = node_text(prop, source);
                    if obj_name.is_empty() {
                        prop_name
                    } else {
                        format!("{}.{}", obj_name, prop_name)
                    }
                } else {
                    node_text(prop, source)
                }
            } else {
                String::new()
            }
        }
        "call_expression" => {
            // Chained calls: foo().bar()
            if let Some(func) = find_child_by_field(node, "function") {
                get_call_name(func, source)
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

/// Extract type references from a type annotation node.
fn extract_type_refs(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    references: &mut Vec<ReferenceEntry>,
) {
    extract_type_refs_recursive(node, source, file_path, parent_ctx, references, 0);
}

fn extract_type_refs_recursive(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    references: &mut Vec<ReferenceEntry>,
    depth: usize,
) {
    if depth > 50 {
        return;
    }

    match node.kind() {
        "type_identifier" | "identifier" => {
            let name = node_text(node, source);
            if !is_ts_primitive_type(&name) && !name.is_empty() {
                let line = node_line_range(node);
                references.push(ReferenceEntry {
                    file: file_path.to_string(),
                    name,
                    kind: "type_annotation".to_string(),
                    line,
                    caller: parent_ctx.map(String::from),
                    project: String::new(),
                });
            }
        }
        "generic_type" | "nested_type_identifier" => {
            // Extract the base type name
            if let Some(name_node) = find_child_by_field(node, "name") {
                let name = node_text(name_node, source);
                if !is_ts_primitive_type(&name) && !name.is_empty() {
                    let line = node_line_range(node);
                    references.push(ReferenceEntry {
                        file: file_path.to_string(),
                        name,
                        kind: "type_annotation".to_string(),
                        line,
                        caller: parent_ctx.map(String::from),
                        project: String::new(),
                    });
                }
            }
            // Recurse into type arguments
            if let Some(args) = find_child_by_field(node, "type_arguments") {
                let mut cursor = args.walk();
                for child in args.children(&mut cursor) {
                    extract_type_refs_recursive(
                        child,
                        source,
                        file_path,
                        parent_ctx,
                        references,
                        depth + 1,
                    );
                }
            }
        }
        _ => {
            // Recurse into children
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                extract_type_refs_recursive(
                    child,
                    source,
                    file_path,
                    parent_ctx,
                    references,
                    depth + 1,
                );
            }
        }
    }
}

// --- Shared JS-like extraction (adapted for TS node names) ---

#[allow(clippy::too_many_arguments)]
fn extract_function_decl(
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

    let line = node_line_range(node);
    let _sig = build_function_signature(node, source, &name);

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

    // Extract tokens from function body
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| filter_ts_tokens(extract_tokens(body, source)));

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        kind,
        line,
        parent_ctx,
        tokens,
        None,
        Some(visibility.to_string()),
    );

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

#[allow(clippy::too_many_arguments)]
fn extract_class(
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

    let line = node_line_range(node);
    let is_exported = node
        .parent()
        .map(|p| p.kind() == "export_statement")
        .unwrap_or(false);
    let visibility = if is_exported { "public" } else { "private" };

    let is_abstract = node.kind() == "abstract_class_declaration";
    let _sig = build_class_signature(node, source, &name, is_abstract);

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
    };

    // Extract type references from class heritage (extends/implements)
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "class_heritage" {
            extract_type_refs(child, source, file_path, Some(&full_name), references);
        }
    }

    // Extract tokens from class body
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| filter_ts_tokens(extract_tokens(body, source)));

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        "class",
        line,
        parent_ctx,
        tokens,
        None,
        Some(visibility.to_string()),
    );

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

#[allow(clippy::too_many_arguments)]
fn extract_method(
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
    let _sig = format!("{prefix}{name}{params}{return_type}");

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

    // Extract tokens from method body
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| filter_ts_tokens(extract_tokens(body, source)));

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        kind,
        line,
        parent_ctx,
        tokens,
        None,
        Some(visibility.to_string()),
    );

    // Recurse into method body with method name as context
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

                // Extract tokens from variable value
                let tokens = value_node.and_then(|v| filter_ts_tokens(extract_tokens(v, source)));

                push_symbol(
                    symbols,
                    file_path,
                    full_name,
                    kind,
                    line,
                    parent_ctx,
                    tokens,
                    None,
                    Some(visibility.to_string()),
                );
            }
        }
    }
}

fn extract_import(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
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
                            Some(name.clone()),
                            Some("private".to_string()),
                        );
                        // Also add import reference
                        references.push(ReferenceEntry {
                            file: file_path.to_string(),
                            name: source_module.clone(),
                            kind: "import".to_string(),
                            line,
                            caller: None,
                            project: String::new(),
                        });
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
                                        full.clone(),
                                        "import",
                                        line,
                                        None,
                                        None,
                                        alias,
                                        Some("private".to_string()),
                                    );
                                    // Also add import reference
                                    references.push(ReferenceEntry {
                                        file: file_path.to_string(),
                                        name: full,
                                        kind: "import".to_string(),
                                        line,
                                        caller: None,
                                        project: String::new(),
                                    });
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
                            full.clone(),
                            "import",
                            line,
                            None,
                            None,
                            alias,
                            Some("private".to_string()),
                        );
                        // Also add import reference
                        references.push(ReferenceEntry {
                            file: file_path.to_string(),
                            name: full,
                            kind: "import".to_string(),
                            line,
                            caller: None,
                            project: String::new(),
                        });
                    }
                    _ => {}
                }
            }
        }
    }
}

// --- TS-specific constructs ---

#[allow(clippy::too_many_arguments)]
fn extract_interface(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
    references: &mut Vec<ReferenceEntry>,
    _depth: usize,
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

    let extends_node = find_child_by_field(node, "extends_type").or_else(|| {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|c| c.kind() == "extends_type_clause")
    });

    let extends = extends_node
        .map(|n| format!(" extends {}", node_text(n, source)))
        .unwrap_or_default();

    let _sig = format!("interface {name}{type_params}{extends}");

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
    };

    // Extract type references from extends clause
    if let Some(ext) = extends_node {
        extract_type_refs(ext, source, file_path, Some(&full_name), references);
    }

    // Extract tokens from interface body (type references)
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| filter_ts_tokens(extract_tokens(body, source)));

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        "interface",
        line,
        parent_ctx,
        tokens,
        None,
        Some(visibility.to_string()),
    );

    // Walk interface body for method signatures and extract type refs
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
                        // Interface signatures don't have bodies, so no tokens
                        push_symbol(
                            symbols,
                            file_path,
                            format!("{full_name}.{member_name}"),
                            member_kind,
                            member_line,
                            Some(&full_name),
                            None,
                            None,
                            Some("public".to_string()),
                        );
                    }
                    // Extract type refs from member type annotations
                    if let Some(type_ann) = find_child_by_field(child, "type") {
                        extract_type_refs(
                            type_ann,
                            source,
                            file_path,
                            Some(&full_name),
                            references,
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
    references: &mut Vec<ReferenceEntry>,
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

    let _sig = format!("type {name}{type_params}");

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name
    };

    // Extract type references from type definition
    if let Some(value) = find_child_by_field(node, "value") {
        extract_type_refs(value, source, file_path, Some(&full_name), references);
    }

    // Extract tokens from type definition
    let tokens = find_child_by_field(node, "value")
        .and_then(|v| filter_ts_tokens(extract_tokens(v, source)));

    push_symbol(
        symbols,
        file_path,
        full_name,
        "type_alias",
        line,
        parent_ctx,
        tokens,
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

#[allow(clippy::too_many_arguments)]
fn extract_namespace(
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
        project: String::new(),
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
        let (symbols, _texts, _refs) = parse_file(source, "typescript", "test.ts").unwrap();
        assert_eq!(symbols.len(), 2);

        let greet = find_sym(&symbols, "greet");
        assert_eq!(greet.kind, "function");
        // Token extraction is enabled (may be None if body has no tokens after filtering)

        let fetch_fn = find_sym(&symbols, "fetch");
        assert_eq!(fetch_fn.kind, "function");
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
        let (symbols, _texts, _refs) = parse_file(source, "typescript", "test.ts").unwrap();

        let user = find_sym(&symbols, "User");
        assert_eq!(user.kind, "interface");
        assert_eq!(user.visibility.as_deref(), Some("public"));

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
        let (symbols, _texts, _refs) = parse_file(source, "typescript", "test.ts").unwrap();

        let result = find_sym(&symbols, "Result");
        assert_eq!(result.kind, "type_alias");
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
        let (symbols, _texts, _refs) = parse_file(source, "typescript", "test.ts").unwrap();

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
        let (symbols, _texts, _refs) = parse_file(source, "typescript", "test.ts").unwrap();

        let base = find_sym(&symbols, "Base");
        assert_eq!(base.kind, "class");

        let worker = find_sym(&symbols, "Worker");
        assert_eq!(worker.kind, "class");

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
        let (symbols, _texts, _refs) = parse_file(source, "typescript", "test.ts").unwrap();

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
        let (symbols, _texts, _refs) = parse_file(source, "typescript", "test.ts").unwrap();

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
        let (symbols, _texts, _refs) = parse_file(source, "typescript", "test.ts").unwrap();

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

    #[test]
    fn test_ts_call_references() {
        let source = b"function main() {
    const result = helper();
    const data = api.fetchData();
    console.log(result);
}";
        let (_symbols, _texts, refs) = parse_file(source, "typescript", "test.ts").unwrap();

        // Should find helper() call
        let helper_ref = refs.iter().find(|r| r.name == "helper");
        assert!(helper_ref.is_some());
        assert_eq!(helper_ref.unwrap().kind, "call");

        // Should find api.fetchData() call
        let api_ref = refs.iter().find(|r| r.name == "api.fetchData");
        assert!(api_ref.is_some());
        assert_eq!(api_ref.unwrap().kind, "call");

        // Should NOT find console.log (builtin)
        let console_ref = refs.iter().find(|r| r.name == "console.log");
        assert!(console_ref.is_none());
    }

    #[test]
    fn test_ts_import_references() {
        let source = b"import React from 'react';
import { Component, useState } from 'react';
import * as Utils from './utils';";
        let (_symbols, _texts, refs) = parse_file(source, "typescript", "test.ts").unwrap();

        // Should find import references
        let react_ref = refs
            .iter()
            .find(|r| r.name == "react" && r.kind == "import");
        assert!(react_ref.is_some());

        let component_ref = refs
            .iter()
            .find(|r| r.name == "react.Component" && r.kind == "import");
        assert!(component_ref.is_some());

        let utils_ref = refs
            .iter()
            .find(|r| r.name == "./utils.*" && r.kind == "import");
        assert!(utils_ref.is_some());
    }

    #[test]
    fn test_ts_type_references() {
        let source = b"interface User extends BaseUser {
    name: string;
    data: CustomType;
}

type Result<T> = Success<T> | Error;

class MyService implements Service {
    getData(): Promise<User[]> {
        return fetch('/api');
    }
}";
        let (_symbols, _texts, refs) = parse_file(source, "typescript", "test.ts").unwrap();

        // Should find type annotation references
        let base_user_ref = refs
            .iter()
            .find(|r| r.name == "BaseUser" && r.kind == "type_annotation");
        assert!(base_user_ref.is_some());

        let custom_type_ref = refs
            .iter()
            .find(|r| r.name == "CustomType" && r.kind == "type_annotation");
        assert!(custom_type_ref.is_some());

        // Should find Success from type alias
        let success_ref = refs
            .iter()
            .find(|r| r.name == "Success" && r.kind == "type_annotation");
        assert!(success_ref.is_some());

        // Service from class implements
        let service_ref = refs
            .iter()
            .find(|r| r.name == "Service" && r.kind == "type_annotation");
        assert!(service_ref.is_some());
    }

    #[test]
    fn test_ts_instantiation_references() {
        let source = b"function createUser() {
    const user = new UserModel('test');
    const date = new Date();
    return user;
}";
        let (_symbols, _texts, refs) = parse_file(source, "typescript", "test.ts").unwrap();

        // Should find UserModel instantiation
        let user_ref = refs
            .iter()
            .find(|r| r.name == "UserModel" && r.kind == "instantiation");
        assert!(user_ref.is_some());

        // Should NOT find Date instantiation (builtin)
        let date_ref = refs
            .iter()
            .find(|r| r.name == "Date" && r.kind == "instantiation");
        assert!(date_ref.is_none());
    }
}
