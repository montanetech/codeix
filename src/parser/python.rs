//! Python symbol and text extraction.

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
        "function_definition" => {
            extract_function(node, source, file_path, parent_ctx, symbols, texts, depth);
            return; // handled recursively
        }
        "class_definition" => {
            extract_class(node, source, file_path, parent_ctx, symbols, texts, depth);
            return; // handled recursively
        }
        "import_statement" => {
            extract_import(node, source, file_path, symbols);
        }
        "import_from_statement" => {
            extract_import_from(node, source, file_path, symbols);
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
            depth + 1,
        );
    }
}

fn extract_function(
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

    // Determine if it's a method (inside a class) or function
    let kind = if parent_ctx.is_some() {
        "method"
    } else {
        "function"
    };

    // Check for decorators to detect properties, staticmethods, etc.
    let visibility = detect_python_visibility(&name);

    // Build signature from parameters
    let sig = build_function_signature(node, source, &name);

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
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
        Some(visibility),
    );

    // Recurse into function body for nested definitions
    if let Some(body) = find_child_by_field(node, "body") {
        // Check for docstring as first statement
        let mut cursor = body.walk();
        let mut first = true;
        for child in body.children(&mut cursor) {
            if first && child.kind() == "expression_statement" {
                if let Some(str_node) = child.child(0)
                    && (str_node.kind() == "string" || str_node.kind() == "concatenated_string")
                {
                    let ctx_name = if let Some(ctx) = parent_ctx {
                        format!("{}.{}", ctx, name)
                    } else {
                        name.clone()
                    };
                    extract_docstring(str_node, source, file_path, Some(&ctx_name), texts);
                }
                first = false;
                continue;
            }
            first = false;
            // Don't recurse deeply into function bodies for symbols,
            // but do recurse for nested classes/functions
            let ctx_name = if let Some(ctx) = parent_ctx {
                format!("{}.{}", ctx, name)
            } else {
                name.clone()
            };
            match child.kind() {
                "function_definition" | "class_definition" | "decorated_definition" => {
                    walk_node(
                        child,
                        source,
                        file_path,
                        Some(&ctx_name),
                        symbols,
                        texts,
                        depth + 1,
                    );
                }
                _ => {}
            }
        }
    }
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
    let visibility = detect_python_visibility(&name);

    // Build signature with base classes
    let sig = build_class_signature(node, source, &name);

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
                depth + 1,
            );
        }
    }
}

fn extract_import(node: Node, source: &[u8], file_path: &str, symbols: &mut Vec<SymbolEntry>) {
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
                    name,
                    "import",
                    line,
                    None,
                    None,
                    None,
                    Some("private".to_string()),
                );
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
                        name,
                        "import",
                        line,
                        None,
                        None,
                        alias,
                        Some("private".to_string()),
                    );
                }
            }
            _ => {}
        }
    }
}

fn extract_import_from(node: Node, source: &[u8], file_path: &str, symbols: &mut Vec<SymbolEntry>) {
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
                    full_import,
                    "import",
                    line,
                    None,
                    None,
                    None,
                    Some("private".to_string()),
                );
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
                        full_import,
                        "import",
                        line,
                        None,
                        None,
                        alias,
                        Some("private".to_string()),
                    );
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
                    full_import,
                    "import",
                    line,
                    None,
                    None,
                    None,
                    Some("private".to_string()),
                );
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

fn build_function_signature(node: Node, source: &[u8], name: &str) -> String {
    let params = find_child_by_field(node, "parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_else(|| "()".to_string());

    let return_type = find_child_by_field(node, "return_type")
        .map(|n| format!(" -> {}", node_text(n, source)))
        .unwrap_or_default();

    // Check for async
    let is_async = node.child(0).map(|c| c.kind() == "async").unwrap_or(false);

    let prefix = if is_async { "async def" } else { "def" };

    format!("{prefix} {name}{params}{return_type}")
}

fn build_class_signature(node: Node, source: &[u8], name: &str) -> String {
    let bases = find_child_by_field(node, "superclasses")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    if bases.is_empty() {
        format!("class {name}")
    } else {
        format!("class {name}{bases}")
    }
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
        let (symbols, _texts) = parse_file(source, "python", "test.py").unwrap();
        assert_eq!(symbols.len(), 3);

        let hello = find_sym(&symbols, "hello");
        assert_eq!(hello.kind, "function");
        assert!(hello.sig.as_ref().unwrap().contains("def hello"));
        assert_eq!(hello.visibility.as_deref(), Some("public"));

        let priv_fn = find_sym(&symbols, "_private");
        assert_eq!(priv_fn.visibility.as_deref(), Some("internal"));

        let async_fn = find_sym(&symbols, "fetch_data");
        assert!(async_fn.sig.as_ref().unwrap().contains("async def"));
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
        let (symbols, _texts) = parse_file(source, "python", "test.py").unwrap();

        let person = find_sym(&symbols, "Person");
        assert_eq!(person.kind, "class");
        assert!(person.sig.as_ref().unwrap().contains("class Person"));
        assert_eq!(person.visibility.as_deref(), Some("public"));

        let init = find_sym(&symbols, "Person.__init__");
        assert_eq!(init.kind, "method");
        assert_eq!(init.parent.as_deref(), Some("Person"));

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
        let (symbols, _texts) = parse_file(source, "python", "test.py").unwrap();

        let os = find_sym(&symbols, "os");
        assert_eq!(os.kind, "import");

        let sys = symbols.iter().find(|s| s.name == "sys").unwrap();
        assert_eq!(sys.alias.as_deref(), Some("system"));

        let path = symbols.iter().find(|s| s.name == "pathlib.Path").unwrap();
        assert_eq!(path.kind, "import");

        let dict = symbols.iter().find(|s| s.name == "typing.Dict").unwrap();
        assert_eq!(dict.alias.as_deref(), Some("D"));
    }

    #[test]
    fn test_python_variables() {
        let source = b"MAX_SIZE = 100
debug_mode = True

class Config:
    def __init__(self):
        self.version = '1.0'";
        let (symbols, _texts) = parse_file(source, "python", "test.py").unwrap();

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
        let (symbols, _texts) = parse_file(source, "python", "test.py").unwrap();

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
        let (_symbols, texts) = parse_file(source, "python", "test.py").unwrap();
        assert!(texts.iter().any(|t| t.kind == "docstring"));
    }
}
