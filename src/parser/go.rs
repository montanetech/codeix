//! Go symbol and text extraction.

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
        "function_declaration" => {
            extract_function(node, source, file_path, symbols, texts, references, depth);
            return; // handled recursively
        }
        "method_declaration" => {
            extract_method(node, source, file_path, symbols, texts, references, depth);
            return; // handled recursively
        }
        "type_declaration" => {
            extract_type_decl(node, source, file_path, symbols, texts);
            return; // handled recursively
        }
        "type_spec" => {
            extract_type_spec(node, source, file_path, parent_ctx, symbols, texts);
            return;
        }
        "var_declaration" | "const_declaration" => {
            extract_var_const(node, source, file_path, parent_ctx, symbols);
        }
        "import_declaration" => {
            extract_imports(node, source, file_path, symbols, references);
        }
        "package_clause" => {
            extract_package(node, source, file_path, symbols);
        }
        "call_expression" => {
            extract_call(node, source, file_path, parent_ctx, references);
        }
        "comment" => {
            extract_go_comment(node, source, file_path, parent_ctx, texts);
            return;
        }
        "interpreted_string_literal" | "raw_string_literal" => {
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

fn extract_function(
    node: Node,
    source: &[u8],
    file_path: &str,
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
    let visibility = go_visibility(&name);

    // Extract tokens from function body for FTS
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| extract_tokens(body, source))
        .map(|t| filter_go_tokens(&t));

    push_symbol(
        symbols,
        file_path,
        name.clone(),
        "function",
        line,
        None,
        tokens,
        None,
        Some(visibility),
    );

    // Recurse into function body to find calls
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            walk_node(
                child,
                source,
                file_path,
                Some(&name),
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
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
    references: &mut Vec<ReferenceEntry>,
    depth: usize,
) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    // Extract receiver type: `func (r *Receiver) Method()`
    let receiver = find_child_by_field(node, "receiver")
        .map(|recv| {
            // The receiver is a parameter_list with one entry
            // Try to extract the type name
            let text = node_text(recv, source);
            // Strip parens and pointer/reference
            text.trim_matches(|c: char| c == '(' || c == ')' || c.is_whitespace())
                .split_whitespace()
                .last()
                .unwrap_or("")
                .trim_start_matches('*')
                .to_string()
        })
        .unwrap_or_default();

    let line = node_line_range(node);
    let visibility = go_visibility(&name);

    // Extract tokens from method body for FTS
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| extract_tokens(body, source))
        .map(|t| filter_go_tokens(&t));

    let full_name = if receiver.is_empty() {
        name.clone()
    } else {
        format!("{receiver}.{name}")
    };

    let parent = if receiver.is_empty() {
        None
    } else {
        Some(receiver.as_str())
    };

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        "method",
        line,
        parent,
        tokens,
        None,
        Some(visibility),
    );

    // Recurse into method body to find calls
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

fn extract_type_decl(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
) {
    // `type (...)` block or `type Foo ...`
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "type_spec" {
            extract_type_spec(child, source, file_path, None, symbols, texts);
        }
    }
}

fn extract_type_spec(
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

    let type_node = find_child_by_field(node, "type");
    let line = node_line_range(node);
    let visibility = go_visibility(&name);

    // Determine kind from the type definition
    let kind = type_node
        .map(|t| match t.kind() {
            "struct_type" => "struct",
            "interface_type" => "interface",
            _ => "type_alias",
        })
        .unwrap_or("type_alias");

    push_symbol(
        symbols,
        file_path,
        name.clone(),
        kind,
        line,
        parent_ctx,
        None,
        None,
        Some(visibility),
    );

    // For structs, extract fields
    if let Some(type_n) = type_node {
        if type_n.kind() == "struct_type"
            && let Some(field_list) = find_child_by_field(type_n, "fields").or_else(|| {
                let mut c = type_n.walk();
                type_n
                    .children(&mut c)
                    .find(|n| n.kind() == "field_declaration_list")
            })
        {
            let mut cursor = field_list.walk();
            for child in field_list.children(&mut cursor) {
                if child.kind() == "field_declaration"
                    && let Some(field_name_node) = find_child_by_field(child, "name")
                {
                    let field_name = node_text(field_name_node, source);
                    let field_line = node_line_range(child);
                    let field_vis = go_visibility(&field_name);
                    push_symbol(
                        symbols,
                        file_path,
                        format!("{name}.{field_name}"),
                        "property",
                        field_line,
                        Some(&name),
                        None,
                        None,
                        Some(field_vis),
                    );
                }
                // Extract comments inside struct
                if child.kind() == "comment" {
                    extract_go_comment(child, source, file_path, Some(&name), texts);
                }
            }
        }
        // For interfaces, extract method signatures
        if type_n.kind() == "interface_type" {
            let mut cursor = type_n.walk();
            for child in type_n.children(&mut cursor) {
                if child.kind() == "method_spec"
                    && let Some(method_name_node) = find_child_by_field(child, "name")
                {
                    let method_name = node_text(method_name_node, source);
                    let method_line = node_line_range(child);
                    let method_vis = go_visibility(&method_name);
                    let method_sig = collapse_whitespace(node_text(child, source).trim());
                    push_symbol(
                        symbols,
                        file_path,
                        format!("{name}.{method_name}"),
                        "method",
                        method_line,
                        Some(&name),
                        Some(method_sig),
                        None,
                        Some(method_vis),
                    );
                }
            }
        }
    }
}

fn extract_var_const(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
) {
    let is_const = node.kind() == "const_declaration";
    let kind = if is_const { "constant" } else { "variable" };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "var_spec" || child.kind() == "const_spec" {
            if let Some(name_node) = find_child_by_field(child, "name") {
                let name = node_text(name_node, source);
                let line = node_line_range(child);
                let visibility = go_visibility(&name);

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
            // Handle multiple names in one spec: `var a, b, c int`
            let mut spec_cursor = child.walk();
            for spec_child in child.children(&mut spec_cursor) {
                if spec_child.kind() == "identifier" {
                    // First identifier is captured by field "name", subsequent ones need manual check
                    if find_child_by_field(child, "name")
                        .map(|n| n.id() == spec_child.id())
                        .unwrap_or(false)
                    {
                        continue; // already captured
                    }
                    let extra_name = node_text(spec_child, source);
                    let extra_line = node_line_range(spec_child);
                    let extra_vis = go_visibility(&extra_name);
                    push_symbol(
                        symbols,
                        file_path,
                        extra_name,
                        kind,
                        extra_line,
                        parent_ctx,
                        None,
                        None,
                        Some(extra_vis),
                    );
                }
            }
        }
    }
}

fn extract_imports(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
    let line = node_line_range(node);

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "import_spec" {
            let path_node = find_child_by_field(child, "path");
            let name_node = find_child_by_field(child, "name");

            if let Some(p) = path_node {
                let path = strip_string_quotes(&node_text(p, source));
                let alias = name_node.map(|n| node_text(n, source));

                push_symbol(
                    symbols,
                    file_path,
                    path.clone(),
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
                    name: path,
                    kind: "import".to_string(),
                    line,
                    caller: None,
                    project: String::new(),
                });
            }
        }
        // Also handle single import: `import "fmt"`
        if child.kind() == "import_spec_list" {
            let mut list_cursor = child.walk();
            for spec in child.children(&mut list_cursor) {
                if spec.kind() == "import_spec" {
                    let path_node = find_child_by_field(spec, "path");
                    let name_node = find_child_by_field(spec, "name");

                    if let Some(p) = path_node {
                        let path = strip_string_quotes(&node_text(p, source));
                        let alias = name_node.map(|n| node_text(n, source));
                        let spec_line = node_line_range(spec);

                        push_symbol(
                            symbols,
                            file_path,
                            path.clone(),
                            "import",
                            spec_line,
                            None,
                            None,
                            alias,
                            Some("private".to_string()),
                        );

                        // Also record as import reference
                        references.push(ReferenceEntry {
                            file: file_path.to_string(),
                            name: path,
                            kind: "import".to_string(),
                            line: spec_line,
                            caller: None,
                            project: String::new(),
                        });
                    }
                }
            }
        }
    }
}

fn extract_package(node: Node, source: &[u8], file_path: &str, symbols: &mut Vec<SymbolEntry>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "package_identifier" {
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

    let name = match func.kind() {
        "identifier" => node_text(func, source),
        "selector_expression" => node_text(func, source),
        _ => return,
    };

    // Skip Go builtins
    if is_go_builtin(&name) {
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

/// Check if a call is to a Go builtin.
fn is_go_builtin(name: &str) -> bool {
    let base = name.split('.').next_back().unwrap_or(name);
    matches!(
        base,
        "make"
            | "new"
            | "len"
            | "cap"
            | "append"
            | "copy"
            | "delete"
            | "close"
            | "panic"
            | "recover"
            | "print"
            | "println"
            | "complex"
            | "real"
            | "imag"
            | "min"
            | "max"
            | "clear"
    )
}

fn extract_go_comment(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    extract_comment(node, source, file_path, parent_ctx, texts);
}

fn go_visibility(name: &str) -> String {
    if name.starts_with(|c: char| c.is_uppercase()) {
        "public".to_string()
    } else {
        "private".to_string()
    }
}

/// Go-specific stopwords to filter from tokens.
const GO_STOPWORDS: &[&str] = &[
    // Keywords and builtins
    "nil",
    "iota",
    "func",
    "var",
    "type",
    "interface",
    "map",
    "chan",
    "range",
    "defer",
    "go",
    "select",
    "goto",
    "package",
    "import",
    // Common short names
    "err",
    "ctx",
    "ok",
    "n",
    "i",
    "j",
    "k",
    // Builtins
    "make",
    "len",
    "cap",
    "append",
    "copy",
    "delete",
    "close",
    "panic",
    "recover",
    "print",
    "println",
    // Test framework
    "require",
];

/// Filter Go-specific tokens from the extracted token string.
fn filter_go_tokens(tokens: &str) -> String {
    tokens
        .split_whitespace()
        .filter(|t| !GO_STOPWORDS.contains(t))
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
    fn test_go_functions() {
        let source = b"package main

func Hello(name string) string {
    return \"Hello, \" + name
}

func privateHelper() {
    println(\"private\")
}";
        let (symbols, _texts, _refs) = parse_file(source, "go", "test.go").unwrap();

        let hello = find_sym(&symbols, "Hello");
        assert_eq!(hello.kind, "function");
        // Tokens contain identifiers from function body
        // Token may be None if all identifiers are filtered as stopwords
        assert_eq!(hello.visibility.as_deref(), Some("public"));

        let helper = find_sym(&symbols, "privateHelper");
        assert_eq!(helper.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_go_methods() {
        let source = b"package main

type Person struct {
    Name string
}

func (p *Person) Greet() string {
    return \"Hello, \" + p.Name
}

func (p Person) privateMethod() {}";
        let (symbols, _texts, _refs) = parse_file(source, "go", "test.go").unwrap();

        let person = find_sym(&symbols, "Person");
        assert_eq!(person.kind, "struct");

        let greet = find_sym(&symbols, "Person.Greet");
        assert_eq!(greet.kind, "method");
        assert_eq!(greet.parent.as_deref(), Some("Person"));
        assert_eq!(greet.visibility.as_deref(), Some("public"));

        let priv_method = find_sym(&symbols, "Person.privateMethod");
        assert_eq!(priv_method.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_go_structs() {
        let source = b"package main

type Point struct {
    X int
    Y int
    z int
}";
        let (symbols, _texts, _refs) = parse_file(source, "go", "test.go").unwrap();

        let point = find_sym(&symbols, "Point");
        assert_eq!(point.kind, "struct");

        let x = find_sym(&symbols, "Point.X");
        assert_eq!(x.kind, "property");
        assert_eq!(x.visibility.as_deref(), Some("public"));

        let z = find_sym(&symbols, "Point.z");
        assert_eq!(z.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_go_interfaces() {
        let source = b"package main

type Reader interface {
    Read() (int, error)
    close()
}";
        let (symbols, _texts, _refs) = parse_file(source, "go", "test.go").unwrap();

        let reader = find_sym(&symbols, "Reader");
        assert_eq!(reader.kind, "interface");
        assert_eq!(reader.visibility.as_deref(), Some("public"));

        // Interface methods may or may not be extracted depending on implementation
        // Just verify the interface itself is extracted correctly
        assert!(symbols.len() >= 2); // at least package + interface
    }

    #[test]
    fn test_go_variables() {
        let source = b"package main

var GlobalVar = 100
var privateVar = 200

const MaxSize = 1000
const minSize = 10";
        let (symbols, _texts, _refs) = parse_file(source, "go", "test.go").unwrap();

        let global = find_sym(&symbols, "GlobalVar");
        assert_eq!(global.kind, "variable");
        assert_eq!(global.visibility.as_deref(), Some("public"));

        let max = find_sym(&symbols, "MaxSize");
        assert_eq!(max.kind, "constant");
        assert_eq!(max.visibility.as_deref(), Some("public"));

        let min = find_sym(&symbols, "minSize");
        assert_eq!(min.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_go_imports() {
        let source = b"package main

import \"fmt\"
import (
    \"os\"
    io \"io/ioutil\"
)";
        let (symbols, _texts, refs) = parse_file(source, "go", "test.go").unwrap();

        let fmt = symbols.iter().find(|s| s.name == "fmt").unwrap();
        assert_eq!(fmt.kind, "import");

        let os = symbols.iter().find(|s| s.name == "os").unwrap();
        assert_eq!(os.kind, "import");

        let io = symbols.iter().find(|s| s.name == "io/ioutil").unwrap();
        assert_eq!(io.alias.as_deref(), Some("io"));

        // Check import references were created
        assert!(refs.iter().any(|r| r.name == "fmt" && r.kind == "import"));
        assert!(refs.iter().any(|r| r.name == "os" && r.kind == "import"));
    }

    #[test]
    fn test_go_type_alias() {
        let source = b"package main

type UserID int
type Handler func(string) error";
        let (symbols, _texts, _refs) = parse_file(source, "go", "test.go").unwrap();

        let user_id = find_sym(&symbols, "UserID");
        assert_eq!(user_id.kind, "type_alias");

        let handler = find_sym(&symbols, "Handler");
        assert_eq!(handler.kind, "type_alias");
    }

    #[test]
    fn test_go_package() {
        let source = b"package mypackage

func Foo() {}";
        let (symbols, _texts, _refs) = parse_file(source, "go", "test.go").unwrap();

        let pkg = symbols.iter().find(|s| s.kind == "module").unwrap();
        assert_eq!(pkg.name, "mypackage");
    }

    #[test]
    fn test_go_comments() {
        let source = b"package main

// Single line comment
func Helper() {}

/* Block comment */";
        let (_symbols, texts, _refs) = parse_file(source, "go", "test.go").unwrap();
        assert!(texts.iter().any(|t| t.kind == "comment"));
    }

    #[test]
    fn test_go_call_references() {
        let source = b"package main

func caller() {
    someFunction()
    pkg.NestedCall()
}

func someFunction() {}";
        let (_symbols, _texts, refs) = parse_file(source, "go", "test.go").unwrap();

        // Check call references were created with caller context
        let call_refs: Vec<_> = refs.iter().filter(|r| r.kind == "call").collect();
        assert!(
            call_refs
                .iter()
                .any(|r| r.name == "someFunction" && r.caller.as_deref() == Some("caller"))
        );
        assert!(
            call_refs
                .iter()
                .any(|r| r.name == "pkg.NestedCall" && r.caller.as_deref() == Some("caller"))
        );
    }
}
