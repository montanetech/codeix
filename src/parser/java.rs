//! Java symbol and text extraction.

use tree_sitter::{Node, Tree};

use crate::index::format::{ReferenceEntry, SymbolEntry, TextEntry};
use crate::parser::helpers::*;
use crate::parser::treesitter::MAX_DEPTH;

/// Java-specific stopwords (keywords and common patterns)
const JAVA_STOPWORDS: &[&str] = &[
    // Keywords
    "null",
    "interface",
    "extends",
    "implements",
    "abstract",
    "final",
    "finally",
    "throws",
    "synchronized",
    "volatile",
    "transient",
    "native",
    "strictfp",
    "instanceof",
    "import",
    "package",
    // Primitive types
    "int",
    "long",
    "short",
    "byte",
    "float",
    "double",
    "boolean",
    "char",
    // Common class names (typically imported)
    "String",
    "Integer",
    "Long",
    "Double",
    "Float",
    "Boolean",
    "Object",
    "System",
    "Exception",
    "RuntimeException",
    "Override",
    "Deprecated",
    "List",
    "Map",
    "Set",
    "ArrayList",
    "HashMap",
    "HashSet",
    // Common variable patterns
    "args",
    "main",
];

/// Filter Java-specific stopwords from extracted tokens.
fn filter_java_tokens(tokens: Option<String>) -> Option<String> {
    tokens.and_then(|t| {
        let filtered: Vec<&str> = t
            .split_whitespace()
            .filter(|tok| !JAVA_STOPWORDS.contains(&tok.to_lowercase().as_str()))
            // Also filter out uppercase-only tokens (likely type names)
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
        "class_declaration" => {
            extract_class(
                node, source, file_path, parent_ctx, "class", symbols, texts, references, depth,
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
                references,
                depth,
            );
            return;
        }
        "enum_declaration" => {
            extract_class(
                node, source, file_path, parent_ctx, "enum", symbols, texts, references, depth,
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
                references,
                depth,
            );
            return;
        }
        "record_declaration" => {
            extract_class(
                node, source, file_path, parent_ctx, "struct", symbols, texts, references, depth,
            );
            return;
        }
        "method_declaration" => {
            let method_name = find_child_by_field(node, "name").map(|n| node_text(n, source));
            extract_method(node, source, file_path, parent_ctx, symbols);
            // Walk method body with method name as parent context
            if let Some(body) = find_child_by_field(node, "body") {
                let full_name = match (parent_ctx, &method_name) {
                    (Some(p), Some(m)) => Some(format!("{p}.{m}")),
                    (None, Some(m)) => Some(m.clone()),
                    _ => None,
                };
                walk_node(
                    body,
                    source,
                    file_path,
                    full_name.as_deref(),
                    symbols,
                    texts,
                    references,
                    depth + 1,
                );
            }
            return;
        }
        "constructor_declaration" => {
            let ctor_name = find_child_by_field(node, "name").map(|n| node_text(n, source));
            extract_constructor(node, source, file_path, parent_ctx, symbols);
            // Walk constructor body
            if let Some(body) = find_child_by_field(node, "body") {
                let full_name = match (parent_ctx, &ctor_name) {
                    (Some(p), Some(m)) => Some(format!("{p}.{m}")),
                    (None, Some(m)) => Some(m.clone()),
                    _ => None,
                };
                walk_node(
                    body,
                    source,
                    file_path,
                    full_name.as_deref(),
                    symbols,
                    texts,
                    references,
                    depth + 1,
                );
            }
            return;
        }
        "field_declaration" => {
            extract_field(node, source, file_path, parent_ctx, symbols);
        }
        "import_declaration" => {
            extract_import(node, source, file_path, symbols, references);
        }
        "package_declaration" => {
            extract_package(node, source, file_path, symbols);
        }
        "method_invocation" => {
            extract_call(node, source, file_path, parent_ctx, references);
        }
        "object_creation_expression" => {
            extract_new_call(node, source, file_path, parent_ctx, references);
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
            references,
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
    references: &mut Vec<ReferenceEntry>,
    depth: usize,
) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let line = node_line_range(node);
    let visibility = extract_java_visibility(node, source);

    // Build signature
    let _sig = build_class_signature(node, source, &name, kind);

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
    };

    // Extract tokens from class body
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| filter_java_tokens(extract_tokens(body, source)));

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
                references,
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
    let _sig = extract_signature_to_brace(node, source);

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name
    };

    // Extract tokens from method body
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| filter_java_tokens(extract_tokens(body, source)));

    push_symbol(
        symbols,
        file_path,
        full_name,
        "method",
        line,
        parent_ctx,
        tokens,
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
    let _sig = extract_signature_to_brace(node, source);

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name
    };

    // Extract tokens from constructor body
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| filter_java_tokens(extract_tokens(body, source)));

    push_symbol(
        symbols,
        file_path,
        full_name,
        "constructor",
        line,
        parent_ctx,
        tokens,
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

fn extract_import(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
    let line = node_line_range(node);

    // Get the import path
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
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
            // Also push as reference
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
}

/// Extract a method invocation reference.
fn extract_call(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    references: &mut Vec<ReferenceEntry>,
) {
    let method_name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    // Get the object if it's a method call on an object
    let name = if let Some(obj) = find_child_by_field(node, "object") {
        let obj_text = node_text(obj, source);
        format!("{obj_text}.{method_name}")
    } else {
        method_name
    };

    if is_java_builtin(&name) {
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

/// Extract an object creation (new) expression reference.
fn extract_new_call(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    references: &mut Vec<ReferenceEntry>,
) {
    let type_node = match find_child_by_field(node, "type") {
        Some(n) => n,
        None => return,
    };

    let name = node_text(type_node, source);

    if is_java_builtin(&name) {
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

/// Check if a name is a Java builtin/common class.
fn is_java_builtin(name: &str) -> bool {
    // Get base name (before any dots)
    let base = name.split('.').next().unwrap_or(name);

    matches!(
        base,
        // System and common output
        "System"
            | "out"
            | "err"
            | "in"
            | "println"
            | "print"
            | "printf"
            // Common classes
            | "String"
            | "Integer"
            | "Long"
            | "Double"
            | "Float"
            | "Boolean"
            | "Character"
            | "Byte"
            | "Short"
            | "Object"
            | "Class"
            // Collections
            | "List"
            | "ArrayList"
            | "LinkedList"
            | "Map"
            | "HashMap"
            | "TreeMap"
            | "Set"
            | "HashSet"
            | "TreeSet"
            | "Collection"
            | "Collections"
            | "Arrays"
            // Exceptions
            | "Exception"
            | "RuntimeException"
            | "IllegalArgumentException"
            | "NullPointerException"
            // Common utility
            | "Math"
            | "Objects"
            | "Optional"
            | "Stream"
            // Primitives (shouldn't appear, but just in case)
            | "int"
            | "long"
            | "double"
            | "float"
            | "boolean"
            | "char"
            | "byte"
            | "short"
            | "void"
    )
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
        let (symbols, _texts, _refs) = parse_file(source, "java", "test.java").unwrap();

        let person = find_sym(&symbols, "Person");
        assert_eq!(person.kind, "class");
        assert_eq!(person.visibility.as_deref(), Some("public"));
        // Token extraction extracts identifiers from class body
        // Token may be None if all identifiers are filtered as stopwords

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
        let (symbols, _texts, _refs) = parse_file(source, "java", "test.java").unwrap();

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
        let (symbols, _texts, _refs) = parse_file(source, "java", "test.java").unwrap();

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
        let (symbols, _texts, _refs) = parse_file(source, "java", "test.java").unwrap();

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
        let (symbols, _texts, _refs) = parse_file(source, "java", "test.java").unwrap();

        let add = find_sym(&symbols, "Calculator.add");
        assert_eq!(add.kind, "method");
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
        let (symbols, _texts, _refs) = parse_file(source, "java", "test.java").unwrap();

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
        let (symbols, _texts, _refs) = parse_file(source, "java", "test.java").unwrap();

        let pkg = symbols.iter().find(|s| s.kind == "module").unwrap();
        assert_eq!(pkg.name, "com.example.app");
    }

    #[test]
    fn test_java_visibility_default() {
        let source = b"class Foo {
    void packagePrivate() {}
}";
        let (symbols, _texts, _refs) = parse_file(source, "java", "test.java").unwrap();

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
        let (_symbols, texts, _refs) = parse_file(source, "java", "test.java").unwrap();
        assert!(texts.iter().any(|t| t.kind == "comment"));
    }

    #[test]
    fn test_java_call_references() {
        let source = b"import com.example.DataService;

class Processor {
    private DataService service;

    public void process() {
        service.fetchData();
        transform(data);
        helper.format(data);
        Parser parser = new Parser();
    }
}";
        let (_symbols, _texts, refs) = parse_file(source, "java", "test.java").unwrap();

        // Check import reference
        let import_ref = refs
            .iter()
            .find(|r| r.name == "com.example.DataService")
            .unwrap();
        assert_eq!(import_ref.kind, "import");

        // Check method call references
        let fetch_call = refs.iter().find(|r| r.name == "service.fetchData").unwrap();
        assert_eq!(fetch_call.kind, "call");
        assert_eq!(fetch_call.caller.as_deref(), Some("Processor.process"));

        let transform_call = refs.iter().find(|r| r.name == "transform").unwrap();
        assert_eq!(transform_call.kind, "call");

        let format_call = refs.iter().find(|r| r.name == "helper.format").unwrap();
        assert_eq!(format_call.kind, "call");

        // Check new expression
        let parser_new = refs.iter().find(|r| r.name == "Parser").unwrap();
        assert_eq!(parser_new.kind, "call");
    }
}
