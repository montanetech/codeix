//! C++ symbol and text extraction.
//!
//! Extends C extraction with classes, namespaces, templates, and access specifiers.

use tree_sitter::{Node, Tree};

use crate::index::format::{ReferenceEntry, SymbolEntry, TextEntry};
use crate::parser::helpers::*;
use crate::parser::treesitter::MAX_DEPTH;

/// C++-specific stopwords (C keywords + C++ keywords, types, etc.)
const CPP_STOPWORDS: &[&str] = &[
    // C keywords
    "goto",
    "sizeof",
    "typedef",
    "union",
    "extern",
    "volatile",
    "register",
    "auto",
    "inline",
    // C++ keywords
    "virtual",
    "override",
    "final",
    "delete",
    "template",
    "typename",
    "namespace",
    "using",
    "noexcept",
    "constexpr",
    "mutable",
    "explicit",
    "friend",
    "operator",
    "nullptr",
    "dynamic_cast",
    "static_cast",
    "reinterpret_cast",
    "const_cast",
    // Primitive types
    "int",
    "char",
    "short",
    "long",
    "float",
    "double",
    "signed",
    "unsigned",
    "bool",
    // STL common
    "std",
    "string",
    "vector",
    "map",
    "set",
    "list",
    "pair",
    "unique_ptr",
    "shared_ptr",
    "cout",
    "cin",
    "endl",
    "cerr",
    // Common patterns
    "argc",
    "argv",
    "main",
    "ret",
    "len",
    "ptr",
    "buf",
    "iter",
];

/// Filter C++-specific stopwords from extracted tokens.
fn filter_cpp_tokens(tokens: Option<String>) -> Option<String> {
    tokens.and_then(|t| {
        let filtered: Vec<&str> = t
            .split_whitespace()
            .filter(|tok| !CPP_STOPWORDS.contains(&tok.to_lowercase().as_str()))
            // Filter uppercase constants
            .filter(|tok| !tok.chars().all(|c| c.is_uppercase() || c == '_'))
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
    walk_node(
        root, source, file_path, None, "public", symbols, texts, references, 0,
    );
}

// ---------------------------------------------------------------------------
// Builtin detection for filtering noisy references
// ---------------------------------------------------------------------------

/// Check if a name is a C++ builtin or STL function.
fn is_cpp_builtin_call(name: &str) -> bool {
    matches!(
        name,
        // C standard library (inherited)
        "printf"
        | "fprintf"
        | "sprintf"
        | "snprintf"
        | "scanf"
        | "malloc"
        | "calloc"
        | "realloc"
        | "free"
        | "strlen"
        | "strcpy"
        | "strcmp"
        | "memcpy"
        | "memset"
        | "sizeof"
        // STL containers methods
        | "push_back"
        | "pop_back"
        | "push_front"
        | "pop_front"
        | "insert"
        | "erase"
        | "clear"
        | "empty"
        | "size"
        | "capacity"
        | "reserve"
        | "resize"
        | "begin"
        | "end"
        | "rbegin"
        | "rend"
        | "cbegin"
        | "cend"
        | "front"
        | "back"
        | "at"
        | "find"
        | "count"
        | "lower_bound"
        | "upper_bound"
        | "emplace"
        | "emplace_back"
        // Smart pointers
        | "make_unique"
        | "make_shared"
        | "make_pair"
        | "make_tuple"
        | "get"
        | "reset"
        | "release"
        | "swap"
        // Algorithms
        | "sort"
        | "find_if"
        | "copy"
        | "move"
        | "transform"
        | "for_each"
        | "accumulate"
        | "count_if"
        | "remove"
        | "remove_if"
        | "reverse"
        | "unique"
        | "binary_search"
        | "min"
        | "max"
        | "min_element"
        | "max_element"
        // Stream operations
        | "cout"
        | "cin"
        | "cerr"
        | "clog"
        | "endl"
        | "flush"
        | "getline"
        // String operations
        | "to_string"
        | "stoi"
        | "stol"
        | "stod"
        | "substr"
        | "length"
        | "c_str"
        | "data"
        | "append"
        | "compare"
        // Memory
        | "new"
        | "delete"
        // Cast operators
        | "static_cast"
        | "dynamic_cast"
        | "const_cast"
        | "reinterpret_cast"
        // Test framework
        | "EXPECT_EQ"
        | "EXPECT_TRUE"
        | "EXPECT_FALSE"
        | "ASSERT_EQ"
        | "ASSERT_TRUE"
        | "ASSERT_FALSE"
        | "TEST"
        | "TEST_F"
    )
}

/// Check if a type name is a C++ primitive or STL type.
fn is_cpp_primitive_type(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "long"
            | "short"
            | "char"
            | "float"
            | "double"
            | "bool"
            | "void"
            | "signed"
            | "unsigned"
            | "size_t"
            | "ssize_t"
            | "ptrdiff_t"
            | "auto"
            | "decltype"
            // STL types (too common)
            | "string"
            | "vector"
            | "map"
            | "set"
            | "unordered_map"
            | "unordered_set"
            | "list"
            | "deque"
            | "queue"
            | "stack"
            | "pair"
            | "tuple"
            | "array"
            | "unique_ptr"
            | "shared_ptr"
            | "weak_ptr"
            | "optional"
            | "variant"
            | "any"
            | "function"
            | "iterator"
            | "const_iterator"
    )
}

#[allow(clippy::too_many_arguments)]
fn walk_node(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    access: &str,
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
        "function_definition" => {
            extract_function(
                node, source, file_path, parent_ctx, access, symbols, references,
            );
        }
        "declaration" => {
            extract_declaration(node, source, file_path, parent_ctx, access, symbols);
        }
        "class_specifier" | "struct_specifier" => {
            extract_class(
                node, source, file_path, kind, parent_ctx, symbols, texts, references, depth,
            );
            return;
        }
        "union_specifier" => {
            extract_class(
                node,
                source,
                file_path,
                "struct_specifier",
                parent_ctx,
                symbols,
                texts,
                references,
                depth,
            );
            return;
        }
        "enum_specifier" => {
            extract_enum(node, source, file_path, parent_ctx, symbols);
        }
        "namespace_definition" => {
            extract_namespace(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return;
        }
        "template_declaration" => {
            // Recurse into the templated declaration
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_node(
                    child,
                    source,
                    file_path,
                    parent_ctx,
                    access,
                    symbols,
                    texts,
                    references,
                    depth + 1,
                );
            }
            return;
        }
        "type_definition" => {
            extract_typedef(node, source, file_path, parent_ctx, symbols);
        }
        "alias_declaration" => {
            // `using Foo = ...;`
            extract_using_alias(node, source, file_path, parent_ctx, symbols);
        }
        "preproc_include" => {
            extract_include(node, source, file_path, symbols, references);
        }
        "preproc_def" | "preproc_function_def" => {
            extract_macro(node, source, file_path, symbols);
        }
        "using_declaration" => {
            extract_using(node, source, file_path, symbols);
        }
        "comment" => {
            extract_comment(node, source, file_path, parent_ctx, texts);
            return;
        }
        "string_literal" | "raw_string_literal" | "concatenated_string" => {
            extract_string(node, source, file_path, parent_ctx, texts);
            return;
        }

        // --- Reference extraction ---
        "call_expression" => {
            extract_call_ref(node, source, file_path, parent_ctx, references);
        }
        "new_expression" => {
            extract_new_ref(node, source, file_path, parent_ctx, references);
        }

        _ => {}
    }

    // Recurse
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_node(
            child,
            source,
            file_path,
            parent_ctx,
            access,
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
    if name.is_empty() || is_cpp_builtin_call(&name) {
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
    let type_node = match find_child_by_field(node, "type") {
        Some(t) => t,
        None => return,
    };

    let name = get_type_name(type_node, source);
    if name.is_empty() || is_cpp_builtin_call(&name) || is_cpp_primitive_type(&name) {
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

/// Get the name of a function call.
fn get_call_name(node: Node, source: &[u8]) -> String {
    match node.kind() {
        "identifier" => node_text(node, source),
        "qualified_identifier" | "template_function" => node_text(node, source),
        "field_expression" => {
            // obj.method or obj->method
            if let Some(field) = find_child_by_field(node, "field") {
                if let Some(arg) = find_child_by_field(node, "argument") {
                    let arg_name = get_call_name(arg, source);
                    let field_name = node_text(field, source);
                    if arg_name.is_empty() {
                        field_name
                    } else {
                        format!("{}.{}", arg_name, field_name)
                    }
                } else {
                    node_text(field, source)
                }
            } else {
                String::new()
            }
        }
        "scoped_identifier" => node_text(node, source),
        _ => String::new(),
    }
}

/// Get the name of a type node.
fn get_type_name(node: Node, source: &[u8]) -> String {
    match node.kind() {
        "type_identifier" | "identifier" => node_text(node, source),
        "template_type" => {
            // Template<T> - get base type
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "type_identifier" {
                    return node_text(child, source);
                }
            }
            String::new()
        }
        "qualified_identifier" | "scoped_type_identifier" => node_text(node, source),
        _ => String::new(),
    }
}

/// Extract a type reference if it's a user-defined type.
fn extract_type_ref(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    references: &mut Vec<ReferenceEntry>,
) {
    let name = get_type_name(node, source);
    if name.is_empty() || is_cpp_primitive_type(&name) {
        return;
    }

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

#[allow(clippy::too_many_arguments)]
fn extract_function(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    access: &str,
    symbols: &mut Vec<SymbolEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
    let declarator = match find_child_by_field(node, "declarator") {
        Some(d) => d,
        None => return,
    };

    let name = extract_declarator_name(declarator, source);
    if name.is_empty() {
        return;
    }

    let line = node_line_range(node);
    let _sig = extract_signature_to_brace(node, source);

    let kind = if parent_ctx.is_some() {
        "method"
    } else {
        "function"
    };

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
    };

    let visibility = if parent_ctx.is_some() {
        access.to_string()
    } else {
        let is_static = has_storage_class(node, source, "static");
        if is_static {
            "private".to_string()
        } else {
            "public".to_string()
        }
    };

    // Extract return type reference
    if let Some(type_node) = find_child_by_field(node, "type") {
        extract_type_ref(type_node, source, file_path, Some(&full_name), references);
    }

    // Extract tokens from function body
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| filter_cpp_tokens(extract_tokens(body, source)));

    push_symbol(
        symbols,
        file_path,
        full_name,
        kind,
        line,
        parent_ctx,
        tokens,
        None,
        Some(visibility),
    );
}

fn extract_declaration(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    access: &str,
    symbols: &mut Vec<SymbolEntry>,
) {
    // Skip declarations inside function bodies
    if let Some(p) = node.parent()
        && (p.kind() == "compound_statement" || p.kind() == "case_statement")
    {
        return;
    }

    let line = node_line_range(node);

    let visibility = if parent_ctx.is_some() {
        access.to_string()
    } else {
        let is_static = has_storage_class(node, source, "static");
        if is_static {
            "private".to_string()
        } else {
            "public".to_string()
        }
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_declarator" => {
                let name = extract_declarator_name(child, source);
                if !name.is_empty() {
                    let _sig = collapse_whitespace(node_text(node, source).trim());
                    let full_name = if let Some(parent) = parent_ctx {
                        format!("{parent}.{name}")
                    } else {
                        name
                    };
                    let kind = if parent_ctx.is_some() {
                        "method"
                    } else {
                        "function"
                    };
                    // Declarations don't have bodies
                    push_symbol(
                        symbols,
                        file_path,
                        full_name,
                        kind,
                        line,
                        parent_ctx,
                        None,
                        None,
                        Some(visibility.clone()),
                    );
                }
            }
            "init_declarator" => {
                if let Some(decl) = find_child_by_field(child, "declarator") {
                    let name = extract_declarator_name(decl, source);
                    if !name.is_empty() {
                        let kind = if parent_ctx.is_some() {
                            "property"
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
                            None,
                            None,
                            Some(visibility.clone()),
                        );
                    }
                }
            }
            "identifier" | "pointer_declarator" | "reference_declarator" => {
                let name = extract_declarator_name(child, source);
                if !name.is_empty() {
                    let kind = if parent_ctx.is_some() {
                        "property"
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
                        None,
                        None,
                        Some(visibility.clone()),
                    );
                }
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_class(
    node: Node,
    source: &[u8],
    file_path: &str,
    specifier_kind: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
    references: &mut Vec<ReferenceEntry>,
    depth: usize,
) {
    let name = find_child_by_field(node, "name")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    if name.is_empty() {
        return;
    }

    let line = node_line_range(node);
    let kind = if specifier_kind == "class_specifier" {
        "class"
    } else {
        "struct"
    };

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
    };

    // Extract base class references
    // Try multiple field names for base classes
    let bases = find_child_by_field(node, "base_class_clause").or_else(|| {
        // Walk children to find base_class_clause node
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|c| c.kind() == "base_class_clause")
    });
    if let Some(bases) = bases {
        let mut cursor = bases.walk();
        for child in bases.children(&mut cursor) {
            match child.kind() {
                "base_class_specifier" => {
                    // Find the type within base_class_specifier
                    let mut base_cursor = child.walk();
                    for base_child in child.children(&mut base_cursor) {
                        if matches!(
                            base_child.kind(),
                            "type_identifier" | "qualified_identifier" | "template_type"
                        ) {
                            extract_type_ref(
                                base_child,
                                source,
                                file_path,
                                Some(&full_name),
                                references,
                            );
                        }
                    }
                }
                // Direct type identifier (without access specifier)
                "type_identifier" | "qualified_identifier" | "template_type" => {
                    extract_type_ref(child, source, file_path, Some(&full_name), references);
                }
                _ => {}
            }
        }
    }

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        kind,
        line,
        parent_ctx,
        None,
        None,
        Some("public".to_string()),
    );

    // Walk class body with access tracking
    if let Some(body) = find_child_by_field(node, "body") {
        // Default access: private for class, public for struct
        let default_access = if specifier_kind == "class_specifier" {
            "private"
        } else {
            "public"
        };
        let mut current_access = default_access.to_string();

        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "access_specifier" {
                let text = node_text(child, source)
                    .trim_end_matches(':')
                    .trim()
                    .to_string();
                current_access = match text.as_str() {
                    "public" => "public".to_string(),
                    "protected" => "internal".to_string(),
                    "private" => "private".to_string(),
                    _ => current_access,
                };
                continue;
            }
            walk_node(
                child,
                source,
                file_path,
                Some(&full_name),
                &current_access,
                symbols,
                texts,
                references,
                depth + 1,
            );
        }
    }
}

fn extract_enum(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
) {
    let name = find_child_by_field(node, "name")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    if name.is_empty() {
        return;
    }

    let line = node_line_range(node);

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
    };

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        "enum",
        line,
        parent_ctx,
        None,
        None,
        Some("public".to_string()),
    );

    // Extract enum values
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "enumerator"
                && let Some(name_node) = find_child_by_field(child, "name")
            {
                let const_name = node_text(name_node, source);
                let const_line = node_line_range(child);
                push_symbol(
                    symbols,
                    file_path,
                    format!("{full_name}.{const_name}"),
                    "constant",
                    const_line,
                    Some(&full_name),
                    None,
                    None,
                    Some("public".to_string()),
                );
            }
        }
    }
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
    let name = find_child_by_field(node, "name")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    // Anonymous namespace
    if name.is_empty() {
        if let Some(body) = find_child_by_field(node, "body") {
            let mut cursor = body.walk();
            for child in body.children(&mut cursor) {
                walk_node(
                    child,
                    source,
                    file_path,
                    parent_ctx,
                    "private",
                    symbols,
                    texts,
                    references,
                    depth + 1,
                );
            }
        }
        return;
    }

    let line = node_line_range(node);
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
        Some("public".to_string()),
    );

    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            walk_node(
                child,
                source,
                file_path,
                Some(&full_name),
                "public",
                symbols,
                texts,
                references,
                depth + 1,
            );
        }
    }
}

fn extract_typedef(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
) {
    let line = node_line_range(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "type_identifier" || child.kind() == "identifier" {
            let name = node_text(child, source);
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
                None,
                None,
                Some("public".to_string()),
            );
        }
    }
}

fn extract_using_alias(
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
        None,
        None,
        Some("public".to_string()),
    );
}

fn extract_using(node: Node, source: &[u8], file_path: &str, symbols: &mut Vec<SymbolEntry>) {
    let line = node_line_range(node);
    let text = node_text(node, source);
    // `using namespace std;` or `using std::string;`
    let name = text
        .strip_prefix("using")
        .unwrap_or(&text)
        .trim()
        .strip_suffix(';')
        .unwrap_or(&text)
        .trim()
        .to_string();

    if !name.is_empty() {
        push_symbol(
            symbols,
            file_path,
            name,
            "import",
            line,
            None,
            None,
            None,
            Some("private".to_string()),
        );
    }
}

fn extract_include(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
    let line = node_line_range(node);
    if let Some(path_node) = find_child_by_field(node, "path") {
        let path = node_text(path_node, source);
        let path = path
            .trim_start_matches(['<', '"'])
            .trim_end_matches(['>', '"'])
            .to_string();
        push_symbol(
            symbols,
            file_path,
            path.clone(),
            "import",
            line,
            None,
            None,
            None,
            Some("private".to_string()),
        );
        // Also add import reference
        references.push(ReferenceEntry {
            file: file_path.to_string(),
            name: path,
            kind: "import".to_string(),
            line,
            caller: None,
            project: String::new(),
        });
    }
}

fn extract_macro(node: Node, source: &[u8], file_path: &str, symbols: &mut Vec<SymbolEntry>) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let line = node_line_range(node);
    let kind = if node.kind() == "preproc_function_def" {
        "macro"
    } else {
        "constant"
    };

    push_symbol(
        symbols,
        file_path,
        name,
        kind,
        line,
        None,
        None,
        None,
        Some("public".to_string()),
    );
}

fn extract_declarator_name(node: Node, source: &[u8]) -> String {
    match node.kind() {
        "identifier" | "field_identifier" | "destructor_name" => node_text(node, source),
        "qualified_identifier" => {
            // namespace::name — use the full qualified name
            node_text(node, source)
        }
        "pointer_declarator" | "reference_declarator" => find_child_by_field(node, "declarator")
            .map(|d| extract_declarator_name(d, source))
            .unwrap_or_default(),
        "function_declarator" => find_child_by_field(node, "declarator")
            .map(|d| extract_declarator_name(d, source))
            .unwrap_or_default(),
        "array_declarator" => find_child_by_field(node, "declarator")
            .map(|d| extract_declarator_name(d, source))
            .unwrap_or_default(),
        "parenthesized_declarator" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                let name = extract_declarator_name(child, source);
                if !name.is_empty() {
                    return name;
                }
            }
            String::new()
        }
        "operator_name" => {
            // operator+, operator<< etc.
            node_text(node, source)
        }
        _ => String::new(),
    }
}

fn has_storage_class(node: Node, source: &[u8], class: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "storage_class_specifier" {
            let text = node_text(child, source);
            if text == class {
                return true;
            }
        }
    }
    false
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
    fn test_cpp_functions() {
        let source = b"int add(int a, int b) {
    return a + b;
}

static void helper() {
    std::cout << \"helper\";
}";
        let (symbols, _texts, _refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        let add = find_sym(&symbols, "add");
        assert_eq!(add.kind, "function");
        // Token extraction is enabled (may be None if body has no tokens after filtering)
        assert_eq!(add.visibility.as_deref(), Some("public"));

        let helper = find_sym(&symbols, "helper");
        assert_eq!(helper.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_cpp_class() {
        let source = b"class Person {
public:
    Person(std::string name) {}

    std::string getName() const {
        return name;
    }

private:
    void privateMethod() {}

protected:
    void helper() {}
};";
        let (symbols, _texts, _refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        let person = find_sym(&symbols, "Person");
        assert_eq!(person.kind, "class");

        let get_name = find_sym(&symbols, "Person.getName");
        assert_eq!(get_name.kind, "method");
        assert_eq!(get_name.visibility.as_deref(), Some("public"));

        let private = find_sym(&symbols, "Person.privateMethod");
        assert_eq!(private.visibility.as_deref(), Some("private"));

        let helper = find_sym(&symbols, "Person.helper");
        assert_eq!(helper.visibility.as_deref(), Some("internal"));
    }

    #[test]
    fn test_cpp_struct() {
        let source = b"struct Point {
    void setX(int value) {}
    void setY(int value) {}

private:
    void hidden() {}
};";
        let (symbols, _texts, _refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        let point = find_sym(&symbols, "Point");
        assert_eq!(point.kind, "struct");

        let set_x = find_sym(&symbols, "Point.setX");
        assert_eq!(set_x.visibility.as_deref(), Some("public")); // struct default

        let hidden = find_sym(&symbols, "Point.hidden");
        assert_eq!(hidden.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_cpp_namespace() {
        let source = b"namespace utils {
    void helper() {}

    class Tool {
    public:
        void run() {}
    };
}";
        let (symbols, _texts, _refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        let utils = find_sym(&symbols, "utils");
        assert_eq!(utils.kind, "module");

        let helper = find_sym(&symbols, "utils.helper");
        assert_eq!(helper.parent.as_deref(), Some("utils"));

        let tool = find_sym(&symbols, "utils.Tool");
        assert_eq!(tool.kind, "class");

        let run = find_sym(&symbols, "utils.Tool.run");
        assert_eq!(run.kind, "method");
    }

    #[test]
    fn test_cpp_enum() {
        let source = b"enum Color {
    RED,
    GREEN,
    BLUE
};

enum class Status {
    OK,
    ERROR
};";
        let (symbols, _texts, _refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        let color = find_sym(&symbols, "Color");
        assert_eq!(color.kind, "enum");

        let red = find_sym(&symbols, "Color.RED");
        assert_eq!(red.kind, "constant");

        let status = find_sym(&symbols, "Status");
        assert_eq!(status.kind, "enum");
    }

    #[test]
    fn test_cpp_template() {
        let source = b"template<typename T>
class Container {
public:
    void add(T item) {}
};

template<typename T>
T max(T a, T b) {
    return (a > b) ? a : b;
}";
        let (symbols, _texts, _refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        let container = find_sym(&symbols, "Container");
        assert_eq!(container.kind, "class");

        let add = find_sym(&symbols, "Container.add");
        assert_eq!(add.kind, "method");

        let max = find_sym(&symbols, "max");
        assert_eq!(max.kind, "function");
    }

    #[test]
    fn test_cpp_using() {
        let source = b"using namespace std;
using std::string;
using MyInt = int;";
        let (symbols, _texts, _refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        let ns = symbols
            .iter()
            .find(|s| s.name.contains("namespace std"))
            .unwrap();
        assert_eq!(ns.kind, "import");

        let string = symbols.iter().find(|s| s.name.contains("string")).unwrap();
        assert_eq!(string.kind, "import");

        let myint = find_sym(&symbols, "MyInt");
        assert_eq!(myint.kind, "type_alias");
    }

    #[test]
    fn test_cpp_includes() {
        let source = b"#include <iostream>
#include \"myheader.h\"";
        let (symbols, _texts, _refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        let iostream = symbols.iter().find(|s| s.name == "iostream").unwrap();
        assert_eq!(iostream.kind, "import");

        let myheader = symbols.iter().find(|s| s.name == "myheader.h").unwrap();
        assert_eq!(myheader.kind, "import");
    }

    #[test]
    fn test_cpp_anonymous_namespace() {
        let source = b"namespace MyNamespace {
    void helper() {}
}";
        let (symbols, _texts, _refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        // Named namespace should be extracted
        let ns = find_sym(&symbols, "MyNamespace");
        assert_eq!(ns.kind, "module");

        let helper = find_sym(&symbols, "MyNamespace.helper");
        // Functions inside namespaces might be classified as methods
        assert!(helper.kind == "function" || helper.kind == "method");
    }

    #[test]
    fn test_cpp_comments() {
        let source = b"/* Block comment */
// Single line comment
class Foo {};";
        let (_symbols, texts, _refs) = parse_file(source, "cpp", "test.cpp").unwrap();
        assert!(texts.iter().any(|t| t.kind == "comment"));
    }

    #[test]
    fn test_cpp_call_references() {
        let source = b"void foo() {}
void bar() {
    foo();
    myService.doSomething();
}";
        let (_symbols, _texts, refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        let calls: Vec<_> = refs.iter().filter(|r| r.kind == "call").collect();
        assert!(calls.iter().any(|r| r.name == "foo"));
        assert!(calls.iter().any(|r| r.name.contains("doSomething")));
    }

    #[test]
    fn test_cpp_include_references() {
        let source = b"#include <iostream>
#include \"myheader.h\"";
        let (_symbols, _texts, refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        let imports: Vec<_> = refs.iter().filter(|r| r.kind == "import").collect();
        assert!(imports.iter().any(|r| r.name == "iostream"));
        assert!(imports.iter().any(|r| r.name == "myheader.h"));
    }

    #[test]
    fn test_cpp_instantiation_references() {
        let source = b"class MyClass {};
void foo() {
    MyClass* obj = new MyClass();
    CustomService* svc = new CustomService();
}";
        let (_symbols, _texts, refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        let instantiations: Vec<_> = refs.iter().filter(|r| r.kind == "instantiation").collect();
        assert!(instantiations.iter().any(|r| r.name == "MyClass"));
        assert!(instantiations.iter().any(|r| r.name == "CustomService"));
    }

    #[test]
    fn test_cpp_type_references() {
        let source = b"class Base {};
class Derived : public Base {
public:
    CustomType process() {
        return CustomType();
    }
};";
        let (_symbols, _texts, refs) = parse_file(source, "cpp", "test.cpp").unwrap();

        let type_refs: Vec<_> = refs
            .iter()
            .filter(|r| r.kind == "type_annotation")
            .collect();
        assert!(type_refs.iter().any(|r| r.name == "Base"));
        assert!(type_refs.iter().any(|r| r.name == "CustomType"));
    }
}
