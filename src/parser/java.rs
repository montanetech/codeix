//! Java symbol and text extraction.

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
        "class_declaration" => {
            extract_class(
                node, source, file_path, parent_ctx, "class", symbols, texts, depth,
            );
            return;
        }
        "interface_declaration" => {
            extract_class(
                node,
                source,
                file_path,
                parent_ctx,
                "interface",
                symbols,
                texts,
                depth,
            );
            return;
        }
        "enum_declaration" => {
            extract_class(
                node, source, file_path, parent_ctx, "enum", symbols, texts, depth,
            );
            return;
        }
        "annotation_type_declaration" => {
            extract_class(
                node,
                source,
                file_path,
                parent_ctx,
                "annotation",
                symbols,
                texts,
                depth,
            );
            return;
        }
        "record_declaration" => {
            extract_class(
                node, source, file_path, parent_ctx, "struct", symbols, texts, depth,
            );
            return;
        }
        "method_declaration" => {
            extract_method(node, source, file_path, parent_ctx, symbols);
        }
        "constructor_declaration" => {
            extract_constructor(node, source, file_path, parent_ctx, symbols);
        }
        "field_declaration" => {
            extract_field(node, source, file_path, parent_ctx, symbols);
        }
        "import_declaration" => {
            extract_import(node, source, file_path, symbols);
        }
        "package_declaration" => {
            extract_package(node, source, file_path, symbols);
        }
        "line_comment" | "block_comment" => {
            extract_java_comment(node, source, file_path, parent_ctx, texts);
            return;
        }
        "string_literal" | "text_block" => {
            extract_string(node, source, file_path, parent_ctx, texts);
            return;
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
            symbols,
            texts,
            depth + 1,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_class(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    kind: &str,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
    depth: usize,
) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let line = node_line_range(node);
    let visibility = extract_java_visibility(node, source);

    // Build signature
    let sig = build_class_signature(node, source, &name, kind);

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
        Some(sig),
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
    let visibility = extract_java_visibility(node, source);
    let sig = extract_signature_to_brace(node, source);

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name
    };

    push_symbol(
        symbols,
        file_path,
        full_name,
        "method",
        line,
        parent_ctx,
        Some(sig),
        None,
        Some(visibility),
    );
}

fn extract_constructor(
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
    let visibility = extract_java_visibility(node, source);
    let sig = extract_signature_to_brace(node, source);

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name
    };

    push_symbol(
        symbols,
        file_path,
        full_name,
        "constructor",
        line,
        parent_ctx,
        Some(sig),
        None,
        Some(visibility),
    );
}

fn extract_field(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
) {
    let line = node_line_range(node);
    let visibility = extract_java_visibility(node, source);

    // Check for static final → constant
    let is_static = has_modifier(node, source, "static");
    let is_final = has_modifier(node, source, "final");

    let kind = if is_static && is_final {
        "constant"
    } else {
        "property"
    };

    // Find declarators
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator"
            && let Some(name_node) = find_child_by_field(child, "name")
        {
            let name = node_text(name_node, source);

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

fn extract_import(node: Node, source: &[u8], file_path: &str, symbols: &mut Vec<SymbolEntry>) {
    let line = node_line_range(node);

    // Get the import path
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
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
    }
}

fn extract_package(node: Node, source: &[u8], file_path: &str, symbols: &mut Vec<SymbolEntry>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
            let name = node_text(child, source);
            let line = node_line_range(node);
            push_symbol(
                symbols,
                file_path,
                name,
                "module",
                line,
                None,
                None,
                None,
                Some("public".to_string()),
            );
        }
    }
}

fn extract_java_comment(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    extract_comment(node, source, file_path, parent_ctx, texts);
}

fn extract_java_visibility(node: Node, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let text = node_text(child, source);
            if text.contains("public") {
                return "public".to_string();
            }
            if text.contains("protected") {
                return "internal".to_string();
            }
            if text.contains("private") {
                return "private".to_string();
            }
            // package-private (no explicit modifier)
            return "internal".to_string();
        }
    }
    "internal".to_string() // default: package-private
}

fn has_modifier(node: Node, source: &[u8], modifier: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let text = node_text(child, source);
            return text.contains(modifier);
        }
    }
    false
}

fn build_class_signature(node: Node, source: &[u8], name: &str, kind: &str) -> String {
    let type_params = find_child_by_field(node, "type_parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    let extends = find_child_by_field(node, "superclass")
        .map(|n| format!(" extends {}", node_text(n, source)))
        .unwrap_or_default();

    let implements = find_child_by_field(node, "interfaces")
        .map(|n| format!(" implements {}", node_text(n, source)))
        .unwrap_or_default();

    format!("{kind} {name}{type_params}{extends}{implements}")
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
    fn test_java_class() {
        let source = b"public class Person {
    private String name;

    public Person(String name) {
        this.name = name;
    }

    public String getName() {
        return name;
    }

    private void helper() {}
}";
        let (symbols, _texts) = parse_file(source, "java", "test.java").unwrap();

        let person = find_sym(&symbols, "Person");
        assert_eq!(person.kind, "class");
        assert_eq!(person.visibility.as_deref(), Some("public"));
        assert!(person.sig.as_ref().unwrap().contains("class Person"));

        let name = find_sym(&symbols, "Person.name");
        assert_eq!(name.kind, "property");
        assert_eq!(name.visibility.as_deref(), Some("private"));

        let constructor = find_sym(&symbols, "Person.Person");
        assert_eq!(constructor.kind, "constructor");

        let get_name = find_sym(&symbols, "Person.getName");
        assert_eq!(get_name.kind, "method");
        assert_eq!(get_name.visibility.as_deref(), Some("public"));

        let helper = find_sym(&symbols, "Person.helper");
        assert_eq!(helper.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_java_interface() {
        let source = b"public interface Runnable {
    void run();
    default void start() {}
}";
        let (symbols, _texts) = parse_file(source, "java", "test.java").unwrap();

        let runnable = find_sym(&symbols, "Runnable");
        assert_eq!(runnable.kind, "interface");
        assert_eq!(runnable.visibility.as_deref(), Some("public"));
    }

    #[test]
    fn test_java_enum() {
        let source = b"public enum Status {
    ACTIVE,
    INACTIVE,
    PENDING
}";
        let (symbols, _texts) = parse_file(source, "java", "test.java").unwrap();

        let status = find_sym(&symbols, "Status");
        assert_eq!(status.kind, "enum");
        assert_eq!(status.visibility.as_deref(), Some("public"));
    }

    #[test]
    fn test_java_fields() {
        let source = b"class Config {
    public static final int MAX_SIZE = 100;
    private int value;
    protected String name;
}";
        let (symbols, _texts) = parse_file(source, "java", "test.java").unwrap();

        let max_size = find_sym(&symbols, "Config.MAX_SIZE");
        assert_eq!(max_size.kind, "constant");
        assert_eq!(max_size.visibility.as_deref(), Some("public"));

        let value = find_sym(&symbols, "Config.value");
        assert_eq!(value.kind, "property");
        assert_eq!(value.visibility.as_deref(), Some("private"));

        let name = find_sym(&symbols, "Config.name");
        assert_eq!(name.visibility.as_deref(), Some("internal"));
    }

    #[test]
    fn test_java_methods() {
        let source = b"class Calculator {
    public int add(int a, int b) {
        return a + b;
    }

    protected double divide(double x, double y) {
        return x / y;
    }

    private void log(String msg) {}
}";
        let (symbols, _texts) = parse_file(source, "java", "test.java").unwrap();

        let add = find_sym(&symbols, "Calculator.add");
        assert_eq!(add.kind, "method");
        assert!(add.sig.as_ref().unwrap().contains("int add"));
        assert_eq!(add.visibility.as_deref(), Some("public"));

        let divide = find_sym(&symbols, "Calculator.divide");
        assert_eq!(divide.visibility.as_deref(), Some("internal"));

        let log = find_sym(&symbols, "Calculator.log");
        assert_eq!(log.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_java_imports() {
        let source = b"import java.util.List;
import java.util.*;
import java.io.File;";
        let (symbols, _texts) = parse_file(source, "java", "test.java").unwrap();

        // Check at least one import is extracted
        let imports: Vec<_> = symbols.iter().filter(|s| s.kind == "import").collect();
        assert!(!imports.is_empty());

        // Check if we have any java.util imports
        let has_util = symbols
            .iter()
            .any(|s| s.name.contains("util") && s.kind == "import");
        assert!(has_util);
    }

    #[test]
    fn test_java_package() {
        let source = b"package com.example.app;

class Foo {}";
        let (symbols, _texts) = parse_file(source, "java", "test.java").unwrap();

        let pkg = symbols.iter().find(|s| s.kind == "module").unwrap();
        assert_eq!(pkg.name, "com.example.app");
    }

    #[test]
    fn test_java_visibility_default() {
        let source = b"class Foo {
    void packagePrivate() {}
}";
        let (symbols, _texts) = parse_file(source, "java", "test.java").unwrap();

        let foo = find_sym(&symbols, "Foo");
        assert_eq!(foo.visibility.as_deref(), Some("internal")); // default = package-private

        let method = find_sym(&symbols, "Foo.packagePrivate");
        assert_eq!(method.visibility.as_deref(), Some("internal"));
    }

    #[test]
    fn test_java_comments() {
        let source = b"/** Javadoc comment */
class Documented {}

// Single line
/* Block comment */";
        let (_symbols, texts) = parse_file(source, "java", "test.java").unwrap();
        assert!(texts.iter().any(|t| t.kind == "comment"));
    }
}
