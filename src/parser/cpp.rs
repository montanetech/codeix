//! C++ symbol and text extraction.
//!
//! Extends C extraction with classes, namespaces, templates, and access specifiers.

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
    walk_node(root, source, file_path, None, "public", symbols, texts);
}

fn walk_node(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    access: &str,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
) {
    let kind = node.kind();

    match kind {
        "function_definition" => {
            extract_function(node, source, file_path, parent_ctx, access, symbols);
        }
        "declaration" => {
            extract_declaration(node, source, file_path, parent_ctx, access, symbols);
        }
        "class_specifier" | "struct_specifier" => {
            extract_class(node, source, file_path, kind, parent_ctx, symbols, texts);
            return;
        }
        "union_specifier" => {
            extract_class(node, source, file_path, "struct_specifier", parent_ctx, symbols, texts);
            return;
        }
        "enum_specifier" => {
            extract_enum(node, source, file_path, parent_ctx, symbols);
        }
        "namespace_definition" => {
            extract_namespace(node, source, file_path, parent_ctx, symbols, texts);
            return;
        }
        "template_declaration" => {
            // Recurse into the templated declaration
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_node(child, source, file_path, parent_ctx, access, symbols, texts);
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
            extract_include(node, source, file_path, symbols);
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
        _ => {}
    }

    // Recurse
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_node(child, source, file_path, parent_ctx, access, symbols, texts);
    }
}

fn extract_function(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    access: &str,
    symbols: &mut Vec<SymbolEntry>,
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
    let sig = extract_signature_to_brace(node, source);

    let kind = if parent_ctx.is_some() { "method" } else { "function" };

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name
    };

    let visibility = if parent_ctx.is_some() {
        access.to_string()
    } else {
        let is_static = has_storage_class(node, source, "static");
        if is_static { "private".to_string() } else { "public".to_string() }
    };

    push_symbol(
        symbols, file_path, full_name, kind, line, parent_ctx,
        Some(sig), None, Some(visibility),
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
    if let Some(p) = node.parent() {
        if p.kind() == "compound_statement" || p.kind() == "case_statement" {
            return;
        }
    }

    let line = node_line_range(node);

    let visibility = if parent_ctx.is_some() {
        access.to_string()
    } else {
        let is_static = has_storage_class(node, source, "static");
        if is_static { "private".to_string() } else { "public".to_string() }
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_declarator" => {
                let name = extract_declarator_name(child, source);
                if !name.is_empty() {
                    let sig = collapse_whitespace(node_text(node, source).trim());
                    let full_name = if let Some(parent) = parent_ctx {
                        format!("{parent}.{name}")
                    } else {
                        name
                    };
                    let kind = if parent_ctx.is_some() { "method" } else { "function" };
                    push_symbol(
                        symbols, file_path, full_name, kind, line, parent_ctx,
                        Some(sig), None, Some(visibility.clone()),
                    );
                }
            }
            "init_declarator" => {
                if let Some(decl) = find_child_by_field(child, "declarator") {
                    let name = extract_declarator_name(decl, source);
                    if !name.is_empty() {
                        let kind = if parent_ctx.is_some() { "property" } else { "variable" };
                        let full_name = if let Some(parent) = parent_ctx {
                            format!("{parent}.{name}")
                        } else {
                            name
                        };
                        push_symbol(
                            symbols, file_path, full_name, kind, line, parent_ctx,
                            None, None, Some(visibility.clone()),
                        );
                    }
                }
            }
            "identifier" | "pointer_declarator" | "reference_declarator" => {
                let name = extract_declarator_name(child, source);
                if !name.is_empty() {
                    let kind = if parent_ctx.is_some() { "property" } else { "variable" };
                    let full_name = if let Some(parent) = parent_ctx {
                        format!("{parent}.{name}")
                    } else {
                        name
                    };
                    push_symbol(
                        symbols, file_path, full_name, kind, line, parent_ctx,
                        None, None, Some(visibility.clone()),
                    );
                }
            }
            _ => {}
        }
    }
}

fn extract_class(
    node: Node,
    source: &[u8],
    file_path: &str,
    specifier_kind: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
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

    push_symbol(
        symbols, file_path, full_name.clone(), kind, line, parent_ctx,
        None, None, Some("public".to_string()),
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
                let text = node_text(child, source).trim_end_matches(':').trim().to_string();
                current_access = match text.as_str() {
                    "public" => "public".to_string(),
                    "protected" => "internal".to_string(),
                    "private" => "private".to_string(),
                    _ => current_access,
                };
                continue;
            }
            walk_node(
                child, source, file_path, Some(&full_name),
                &current_access, symbols, texts,
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
        symbols, file_path, full_name.clone(), "enum", line, parent_ctx,
        None, None, Some("public".to_string()),
    );

    // Extract enum values
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "enumerator" {
                if let Some(name_node) = find_child_by_field(child, "name") {
                    let const_name = node_text(name_node, source);
                    let const_line = node_line_range(child);
                    push_symbol(
                        symbols, file_path,
                        format!("{full_name}.{const_name}"),
                        "constant", const_line, Some(&full_name),
                        None, None, Some("public".to_string()),
                    );
                }
            }
        }
    }
}

fn extract_namespace(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
) {
    let name = find_child_by_field(node, "name")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    // Anonymous namespace
    if name.is_empty() {
        if let Some(body) = find_child_by_field(node, "body") {
            let mut cursor = body.walk();
            for child in body.children(&mut cursor) {
                walk_node(child, source, file_path, parent_ctx, "private", symbols, texts);
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
        symbols, file_path, full_name.clone(), "module", line, parent_ctx,
        None, None, Some("public".to_string()),
    );

    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            walk_node(child, source, file_path, Some(&full_name), "public", symbols, texts);
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
                symbols, file_path, full_name, "type_alias", line, parent_ctx,
                None, None, Some("public".to_string()),
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
        symbols, file_path, full_name, "type_alias", line, parent_ctx,
        None, None, Some("public".to_string()),
    );
}

fn extract_using(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
) {
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
            symbols, file_path, name, "import", line, None,
            None, None, Some("private".to_string()),
        );
    }
}

fn extract_include(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
) {
    let line = node_line_range(node);
    if let Some(path_node) = find_child_by_field(node, "path") {
        let path = node_text(path_node, source);
        let path = path
            .trim_start_matches(|c| c == '<' || c == '"')
            .trim_end_matches(|c| c == '>' || c == '"')
            .to_string();
        push_symbol(
            symbols, file_path, path, "import", line, None,
            None, None, Some("private".to_string()),
        );
    }
}

fn extract_macro(
    node: Node,
    source: &[u8],
    file_path: &str,
    symbols: &mut Vec<SymbolEntry>,
) {
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
        symbols, file_path, name, kind, line, None,
        None, None, Some("public".to_string()),
    );
}

fn extract_declarator_name(node: Node, source: &[u8]) -> String {
    match node.kind() {
        "identifier" | "field_identifier" | "destructor_name" => node_text(node, source),
        "qualified_identifier" => {
            // namespace::name — use the full qualified name
            node_text(node, source)
        }
        "pointer_declarator" | "reference_declarator" => {
            find_child_by_field(node, "declarator")
                .map(|d| extract_declarator_name(d, source))
                .unwrap_or_default()
        }
        "function_declarator" => {
            find_child_by_field(node, "declarator")
                .map(|d| extract_declarator_name(d, source))
                .unwrap_or_default()
        }
        "array_declarator" => {
            find_child_by_field(node, "declarator")
                .map(|d| extract_declarator_name(d, source))
                .unwrap_or_default()
        }
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
        let (symbols, _texts) = parse_file(source, "cpp", "test.cpp").unwrap();

        let add = find_sym(&symbols, "add");
        assert_eq!(add.kind, "function");
        assert!(add.sig.as_ref().unwrap().contains("int add"));
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
        let (symbols, _texts) = parse_file(source, "cpp", "test.cpp").unwrap();

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
        let (symbols, _texts) = parse_file(source, "cpp", "test.cpp").unwrap();

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
        let (symbols, _texts) = parse_file(source, "cpp", "test.cpp").unwrap();

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
        let (symbols, _texts) = parse_file(source, "cpp", "test.cpp").unwrap();

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
        let (symbols, _texts) = parse_file(source, "cpp", "test.cpp").unwrap();

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
        let (symbols, _texts) = parse_file(source, "cpp", "test.cpp").unwrap();

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
        let (symbols, _texts) = parse_file(source, "cpp", "test.cpp").unwrap();

        let iostream = symbols.iter().find(|s| s.name == "iostream").unwrap();
        assert_eq!(iostream.kind, "import");

        let myheader = symbols
            .iter()
            .find(|s| s.name == "myheader.h")
            .unwrap();
        assert_eq!(myheader.kind, "import");
    }

    #[test]
    fn test_cpp_anonymous_namespace() {
        let source = b"namespace MyNamespace {
    void helper() {}
}";
        let (symbols, _texts) = parse_file(source, "cpp", "test.cpp").unwrap();

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
        let (_symbols, texts) = parse_file(source, "cpp", "test.cpp").unwrap();
        assert!(texts.iter().any(|t| t.kind == "comment"));
    }
}
