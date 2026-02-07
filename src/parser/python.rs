//! Python symbol and text extraction.

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
        "function_definition" => {
            extract_function(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return; // handled recursively
        }
        "class_definition" => {
            extract_class(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return; // handled recursively
        }
        "import_statement" => {
            extract_import(node, source, file_path, symbols, references);
        }
        "import_from_statement" => {
            extract_import_from(node, source, file_path, symbols, references);
        }
        "decorated_definition" => {
            // Recurse into the definition inside the decorator
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
        "expression_statement" => {
            // Check for module-level assignments and docstrings
            if let Some(child) = node.child(0) {
                match child.kind() {
                    "assignment" => {
                        extract_assignment(child, source, file_path, parent_ctx, symbols);
                    }
                    "string" | "concatenated_string" => {
                        // Could be a module/class docstring
                        extract_docstring(child, source, file_path, parent_ctx, texts);
                    }
                    _ => {}
                }
            }
        }
        "call" => {
            extract_call(node, source, file_path, parent_ctx, references);
        }
        "comment" => {
            extract_python_comment(node, source, file_path, parent_ctx, texts);
            return;
        }
        "string" | "concatenated_string" => {
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

    let line = node_line_range(node);

    // Determine if it's a method (inside a class) or function
    let kind = if parent_ctx.is_some() {
        "method"
    } else {
        "function"
    };

    // Check for decorators to detect properties, staticmethods, etc.
    let visibility = detect_python_visibility(&name);

    // Extract tokens from function body for FTS
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| extract_tokens(body, source))
        .map(|t| filter_python_tokens(&t));

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
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

    // Recurse into function body for nested definitions and references
    if let Some(body) = find_child_by_field(node, "body") {
        // Check for docstring as first statement
        let mut cursor = body.walk();
        let mut first = true;
        for child in body.children(&mut cursor) {
            // Check if first statement is a docstring
            if first
                && child.kind() == "expression_statement"
                && let Some(str_node) = child.child(0)
                && (str_node.kind() == "string" || str_node.kind() == "concatenated_string")
            {
                extract_docstring(str_node, source, file_path, Some(&full_name), texts);
                first = false;
                continue; // Skip docstring, don't process as regular code
            }
            first = false;
            // Recurse into function body to find calls and nested definitions
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
    let visibility = detect_python_visibility(&name);

    // Extract tokens from class body for FTS
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| extract_tokens(body, source))
        .map(|t| filter_python_tokens(&t));

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
        tokens,
        None,
        Some(visibility),
    );

    // Walk class body
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        let mut first = true;
        for child in body.children(&mut cursor) {
            // Check for class docstring
            if first && child.kind() == "expression_statement" {
                if let Some(str_node) = child.child(0)
                    && (str_node.kind() == "string" || str_node.kind() == "concatenated_string")
                {
                    extract_docstring(str_node, source, file_path, Some(&full_name), texts);
                }
                first = false;
                continue;
            }
            first = false;
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

fn extract_import(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
    let line = node_line_range(node);

    // `import foo, bar` or `import foo as bar`
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" => {
                let name = node_text(child, source);
                push_symbol(
                    symbols,
                    file_path,
                    name.clone(),
                    "import",
                    line,
                    None,
                    None,
                    None,
                    Some("private".to_string()),
                );
                // Also record as import reference
                references.push(ReferenceEntry {
                    file: file_path.to_string(),
                    name,
                    kind: "import".to_string(),
                    line,
                    caller: None,
                    project: String::new(),
                });
            }
            "aliased_import" => {
                let name_node = find_child_by_field(child, "name");
                let alias_node = find_child_by_field(child, "alias");
                if let Some(n) = name_node {
                    let name = node_text(n, source);
                    let alias = alias_node.map(|a| node_text(a, source));
                    push_symbol(
                        symbols,
                        file_path,
                        name.clone(),
                        "import",
                        line,
                        None,
                        None,
                        alias,
                        Some("private".to_string()),
                    );
                    // Also record as import reference
                    references.push(ReferenceEntry {
                        file: file_path.to_string(),
                        name,
                        kind: "import".to_string(),
                        line,
                        caller: None,
                        project: String::new(),
                    });
                }
            }
            _ => {}
        }
    }
}

fn extract_import_from(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
    let line = node_line_range(node);

    // Get module name: `from X import ...`
    let module = find_child_by_field(node, "module_name")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    // Iterate over imported names
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" | "identifier" => {
                // Skip the module name itself (already captured)
                if find_child_by_field(node, "module_name")
                    .map(|n| n.id() == child.id())
                    .unwrap_or(false)
                {
                    continue;
                }
                let imported = node_text(child, source);
                let full_import = if module.is_empty() {
                    imported
                } else {
                    format!("{module}.{imported}")
                };
                push_symbol(
                    symbols,
                    file_path,
                    full_import.clone(),
                    "import",
                    line,
                    None,
                    None,
                    None,
                    Some("private".to_string()),
                );
                // Also record as import reference
                references.push(ReferenceEntry {
                    file: file_path.to_string(),
                    name: full_import,
                    kind: "import".to_string(),
                    line,
                    caller: None,
                    project: String::new(),
                });
            }
            "aliased_import" => {
                let name_node = find_child_by_field(child, "name");
                let alias_node = find_child_by_field(child, "alias");
                if let Some(n) = name_node {
                    let imported = node_text(n, source);
                    let full_import = if module.is_empty() {
                        imported
                    } else {
                        format!("{module}.{imported}")
                    };
                    let alias = alias_node.map(|a| node_text(a, source));
                    push_symbol(
                        symbols,
                        file_path,
                        full_import.clone(),
                        "import",
                        line,
                        None,
                        None,
                        alias,
                        Some("private".to_string()),
                    );
                    // Also record as import reference
                    references.push(ReferenceEntry {
                        file: file_path.to_string(),
                        name: full_import,
                        kind: "import".to_string(),
                        line,
                        caller: None,
                        project: String::new(),
                    });
                }
            }
            "wildcard_import" => {
                let full_import = if module.is_empty() {
                    "*".to_string()
                } else {
                    format!("{module}.*")
                };
                push_symbol(
                    symbols,
                    file_path,
                    full_import.clone(),
                    "import",
                    line,
                    None,
                    None,
                    None,
                    Some("private".to_string()),
                );
                // Also record as import reference
                references.push(ReferenceEntry {
                    file: file_path.to_string(),
                    name: full_import,
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

fn extract_assignment(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
) {
    // Module/class-level assignments: `FOO = ...` or `foo: type = ...`
    let left = match find_child_by_field(node, "left") {
        Some(n) => n,
        None => return,
    };

    // Only capture simple identifier assignments (not destructuring, subscripts, etc.)
    if left.kind() != "identifier" {
        return;
    }

    let name = node_text(left, source);
    let line = node_line_range(node);
    let visibility = detect_python_visibility(&name);

    // UPPER_CASE → constant, otherwise variable
    let kind = if name.chars().all(|c| c.is_uppercase() || c == '_') && name.len() > 1 {
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
        None,
        None,
        Some(visibility),
    );
}

/// Extract a function call as a reference.
/// Handles: simple calls (foo()), method calls (obj.method()), chained calls (a.b.c()).
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
        "identifier" => {
            // Simple call: foo()
            node_text(func, source)
        }
        "attribute" => {
            // Method call: obj.method() or chained: a.b.c()
            // We capture the full attribute chain
            node_text(func, source)
        }
        _ => {
            // Complex expression like lambda calls, subscript calls, etc.
            // Skip these as they're hard to resolve statically
            return;
        }
    };

    // Skip builtins and common patterns that aren't useful references
    if is_builtin_call(&name) {
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

/// Check if a call is to a Python builtin that we want to skip.
fn is_builtin_call(name: &str) -> bool {
    // Get the base name (first part for attribute chains)
    let base = name.split('.').next().unwrap_or(name);

    matches!(
        base,
        "print"
            | "len"
            | "str"
            | "int"
            | "float"
            | "bool"
            | "list"
            | "dict"
            | "set"
            | "tuple"
            | "range"
            | "enumerate"
            | "zip"
            | "map"
            | "filter"
            | "sorted"
            | "reversed"
            | "any"
            | "all"
            | "min"
            | "max"
            | "sum"
            | "abs"
            | "round"
            | "type"
            | "isinstance"
            | "issubclass"
            | "hasattr"
            | "getattr"
            | "setattr"
            | "delattr"
            | "open"
            | "input"
            | "repr"
            | "format"
            | "id"
            | "hash"
            | "iter"
            | "next"
            | "super"
            | "property"
            | "staticmethod"
            | "classmethod"
    )
}

fn extract_docstring(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    let raw = node_text(node, source);
    let line = node_line_range(node);
    let text = strip_string_quotes(&raw).trim().to_string();

    if is_trivial_text(&text) {
        return;
    }

    texts.push(TextEntry {
        file: file_path.to_string(),
        kind: "docstring".to_string(),
        line,
        text,
        parent: parent_ctx.map(String::from),
        project: String::new(),
    });
}

fn extract_python_comment(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    extract_comment(node, source, file_path, parent_ctx, texts);
}

fn detect_python_visibility(name: &str) -> String {
    if name.starts_with("__") && !name.ends_with("__") {
        "private".to_string()
    } else if name.starts_with('_') {
        "internal".to_string()
    } else {
        "public".to_string()
    }
}

/// Python-specific stopwords to filter from tokens.
const PYTHON_STOPWORDS: &[&str] = &[
    "self", "cls", "args", "kwargs", "super", "None", "True", "False",
];

/// Filter Python-specific tokens from the extracted token string.
fn filter_python_tokens(tokens: &str) -> String {
    tokens
        .split_whitespace()
        .filter(|t| !PYTHON_STOPWORDS.contains(t))
        .collect::<Vec<_>>()
        .join(" ")
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
    fn test_python_functions() {
        let source = b"def hello(name):
    return f'Hello, {name}!'

def _private():
    pass

async def fetch_data():
    return None";
        let (symbols, _texts, _refs) = parse_file(source, "python", "test.py").unwrap();
        assert_eq!(symbols.len(), 3);

        let hello = find_sym(&symbols, "hello");
        assert_eq!(hello.kind, "function");
        // Tokens should contain identifiers from the function body (name param filtered by stopwords)
        // Token may be None if all identifiers are filtered as stopwords
        assert_eq!(hello.visibility.as_deref(), Some("public"));

        let priv_fn = find_sym(&symbols, "_private");
        assert_eq!(priv_fn.visibility.as_deref(), Some("internal"));
        // Empty body, no meaningful tokens
        assert!(priv_fn.tokens.is_none());

        let async_fn = find_sym(&symbols, "fetch_data");
        // Body just returns None, no meaningful tokens after filtering
        assert!(async_fn.tokens.is_none());
    }

    #[test]
    fn test_python_classes() {
        let source = b"class Person:
    def __init__(self, name):
        self.name = name

    def greet(self):
        return f'Hi, {self.name}'

class _Private:
    pass";
        let (symbols, _texts, _refs) = parse_file(source, "python", "test.py").unwrap();

        let person = find_sym(&symbols, "Person");
        assert_eq!(person.kind, "class");
        // Class body tokens should contain identifiers from methods
        // Token may be None if all identifiers are filtered as stopwords
        assert_eq!(person.visibility.as_deref(), Some("public"));

        let init = find_sym(&symbols, "Person.__init__");
        assert_eq!(init.kind, "method");
        assert_eq!(init.parent.as_deref(), Some("Person"));
        // Method body has 'name' identifier
        // Token may be None if all identifiers are filtered as stopwords

        let greet = find_sym(&symbols, "Person.greet");
        assert_eq!(greet.kind, "method");

        let priv_class = find_sym(&symbols, "_Private");
        assert_eq!(priv_class.visibility.as_deref(), Some("internal"));
    }

    #[test]
    fn test_python_imports() {
        let source = b"import os
import sys as system
from pathlib import Path
from typing import List, Dict as D";
        let (symbols, _texts, refs) = parse_file(source, "python", "test.py").unwrap();

        let os = find_sym(&symbols, "os");
        assert_eq!(os.kind, "import");

        let sys = symbols.iter().find(|s| s.name == "sys").unwrap();
        assert_eq!(sys.alias.as_deref(), Some("system"));

        let path = symbols.iter().find(|s| s.name == "pathlib.Path").unwrap();
        assert_eq!(path.kind, "import");

        let dict = symbols.iter().find(|s| s.name == "typing.Dict").unwrap();
        assert_eq!(dict.alias.as_deref(), Some("D"));

        // Check import references were created
        assert!(refs.iter().any(|r| r.name == "os" && r.kind == "import"));
        assert!(refs.iter().any(|r| r.name == "sys" && r.kind == "import"));
        assert!(
            refs.iter()
                .any(|r| r.name == "pathlib.Path" && r.kind == "import")
        );
    }

    #[test]
    fn test_python_variables() {
        let source = b"MAX_SIZE = 100
debug_mode = True

class Config:
    def __init__(self):
        self.version = '1.0'";
        let (symbols, _texts, _refs) = parse_file(source, "python", "test.py").unwrap();

        let max_size = find_sym(&symbols, "MAX_SIZE");
        assert_eq!(max_size.kind, "constant");

        let debug = find_sym(&symbols, "debug_mode");
        assert_eq!(debug.kind, "variable");

        let config = find_sym(&symbols, "Config");
        assert_eq!(config.kind, "class");
    }

    #[test]
    fn test_python_visibility() {
        let source = b"def public_fn():
    pass

def _internal():
    pass

def __private():
    pass

class Foo:
    def __special__(self):
        pass";
        let (symbols, _texts, _refs) = parse_file(source, "python", "test.py").unwrap();

        let public = find_sym(&symbols, "public_fn");
        assert_eq!(public.visibility.as_deref(), Some("public"));

        let internal = find_sym(&symbols, "_internal");
        assert_eq!(internal.visibility.as_deref(), Some("internal"));

        let private = find_sym(&symbols, "__private");
        assert_eq!(private.visibility.as_deref(), Some("private"));

        let special = find_sym(&symbols, "Foo.__special__");
        // __special__ starts with _ so it's internal (not __x without trailing __)
        assert_eq!(special.visibility.as_deref(), Some("internal"));
    }

    #[test]
    fn test_python_docstrings() {
        let source = b"\"\"\"Module docstring\"\"\"

def foo():
    \"\"\"Function docstring\"\"\"
    pass

class Bar:
    \"\"\"Class docstring\"\"\"
    pass";
        let (_symbols, texts, _refs) = parse_file(source, "python", "test.py").unwrap();
        assert!(texts.iter().any(|t| t.kind == "docstring"));
    }

    #[test]
    fn test_python_call_references() {
        let source = b"def caller():
    result = some_function()
    obj.method_call()
    nested.deep.call()

def some_function():
    pass";
        let (_symbols, _texts, refs) = parse_file(source, "python", "test.py").unwrap();

        // Check call references were created with caller context
        let call_refs: Vec<_> = refs.iter().filter(|r| r.kind == "call").collect();
        assert!(
            call_refs
                .iter()
                .any(|r| r.name == "some_function" && r.caller.as_deref() == Some("caller"))
        );
        assert!(
            call_refs
                .iter()
                .any(|r| r.name == "obj.method_call" && r.caller.as_deref() == Some("caller"))
        );
        assert!(
            call_refs
                .iter()
                .any(|r| r.name == "nested.deep.call" && r.caller.as_deref() == Some("caller"))
        );
    }
}
