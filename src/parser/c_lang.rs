//! C symbol and text extraction.

use tree_sitter::{Node, Tree};

use crate::index::format::{ReferenceEntry, SymbolEntry, TextEntry};
use crate::parser::helpers::*;
use crate::parser::treesitter::MAX_DEPTH;

/// C-specific stopwords (keywords, common types, etc.)
const C_STOPWORDS: &[&str] = &[
    // Keywords
    "goto",
    "sizeof",
    "typedef",
    "union",
    "extern",
    "volatile",
    "register",
    "auto",
    "inline",
    // Primitive types
    "int",
    "char",
    "short",
    "long",
    "float",
    "double",
    "signed",
    "unsigned",
    // Common standard types
    "size_t",
    "ssize_t",
    "ptrdiff_t",
    "FILE",
    // Common macros and constants
    "NULL",
    "EOF",
    // Common variable patterns
    "argc",
    "argv",
    "main",
    "ret",
    "len",
    "ptr",
    "buf",
];

/// Filter C-specific stopwords from extracted tokens.
fn filter_c_tokens(tokens: Option<String>) -> Option<String> {
    tokens.and_then(|t| {
        let filtered: Vec<&str> = t
            .split_whitespace()
            .filter(|tok| !C_STOPWORDS.contains(&tok.to_lowercase().as_str()))
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
    walk_node(root, source, file_path, None, symbols, texts, references, 0);
}

// ---------------------------------------------------------------------------
// Builtin detection for filtering noisy references
// ---------------------------------------------------------------------------

/// Check if a function name is a C standard library function.
fn is_c_builtin_call(name: &str) -> bool {
    matches!(
        name,
        // stdio.h
        "printf"
        | "fprintf"
        | "sprintf"
        | "snprintf"
        | "scanf"
        | "fscanf"
        | "sscanf"
        | "fopen"
        | "fclose"
        | "fread"
        | "fwrite"
        | "fgets"
        | "fputs"
        | "getc"
        | "putc"
        | "getchar"
        | "putchar"
        | "fflush"
        | "fseek"
        | "ftell"
        | "rewind"
        | "feof"
        | "ferror"
        | "perror"
        | "remove"
        | "rename"
        // stdlib.h
        | "malloc"
        | "calloc"
        | "realloc"
        | "free"
        | "exit"
        | "abort"
        | "atexit"
        | "atoi"
        | "atol"
        | "atof"
        | "strtol"
        | "strtoul"
        | "strtod"
        | "rand"
        | "srand"
        | "qsort"
        | "bsearch"
        | "abs"
        | "labs"
        | "div"
        | "ldiv"
        | "getenv"
        | "system"
        // string.h
        | "strlen"
        | "strcpy"
        | "strncpy"
        | "strcat"
        | "strncat"
        | "strcmp"
        | "strncmp"
        | "strchr"
        | "strrchr"
        | "strstr"
        | "strtok"
        | "memcpy"
        | "memmove"
        | "memset"
        | "memcmp"
        | "memchr"
        // ctype.h
        | "isalpha"
        | "isdigit"
        | "isalnum"
        | "isspace"
        | "isupper"
        | "islower"
        | "toupper"
        | "tolower"
        // math.h
        | "sin"
        | "cos"
        | "tan"
        | "asin"
        | "acos"
        | "atan"
        | "atan2"
        | "sinh"
        | "cosh"
        | "tanh"
        | "exp"
        | "log"
        | "log10"
        | "pow"
        | "sqrt"
        | "ceil"
        | "floor"
        | "fabs"
        | "fmod"
        // assert.h
        | "assert"
        // Common patterns
        | "sizeof"
        | "offsetof"
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
        "function_definition" => {
            extract_function(node, source, file_path, symbols, references);
        }
        "declaration" => {
            extract_declaration(node, source, file_path, parent_ctx, symbols);
        }
        "struct_specifier" => {
            extract_struct_or_union(
                node, source, file_path, "struct", parent_ctx, symbols, texts, references,
            );
            return;
        }
        "union_specifier" => {
            extract_struct_or_union(
                node, source, file_path, "struct", parent_ctx, symbols, texts, references,
            );
            return;
        }
        "enum_specifier" => {
            extract_enum(node, source, file_path, parent_ctx, symbols);
        }
        "type_definition" => {
            extract_typedef(node, source, file_path, symbols);
        }
        "preproc_include" => {
            extract_include(node, source, file_path, symbols, references);
        }
        "preproc_def" | "preproc_function_def" => {
            extract_macro(node, source, file_path, symbols);
        }
        "comment" => {
            extract_comment(node, source, file_path, parent_ctx, texts);
            return;
        }
        "string_literal" | "concatenated_string" => {
            extract_string(node, source, file_path, parent_ctx, texts);
            return;
        }

        // --- Reference extraction ---
        "call_expression" => {
            extract_call_ref(node, source, file_path, parent_ctx, references);
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
    if name.is_empty() || is_c_builtin_call(&name) {
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

/// Get the name of a function call.
fn get_call_name(node: Node, source: &[u8]) -> String {
    match node.kind() {
        "identifier" => node_text(node, source),
        "field_expression" => {
            // obj->method or obj.method
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
        "parenthesized_expression" => {
            // (*func_ptr)()
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                let name = get_call_name(child, source);
                if !name.is_empty() {
                    return name;
                }
            }
            String::new()
        }
        "pointer_expression" => {
            // *func_ptr
            if let Some(arg) = find_child_by_field(node, "argument") {
                get_call_name(arg, source)
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

fn extract_function(
    node: Node,
    source: &[u8],
    file_path: &str,
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

    // Check for static (file-scoped)
    let is_static = has_storage_class(node, source, "static");
    let visibility = if is_static { "private" } else { "public" };

    let _sig = extract_signature_to_brace(node, source);

    // Extract return type reference (if not primitive)
    if let Some(type_node) = find_child_by_field(node, "type") {
        extract_type_ref(type_node, source, file_path, Some(&name), references);
    }

    // Extract tokens from function body
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| filter_c_tokens(extract_tokens(body, source)));

    push_symbol(
        symbols,
        file_path,
        name,
        "function",
        line,
        None,
        tokens,
        None,
        Some(visibility.to_string()),
    );
}

/// Extract a type reference if it's a user-defined type.
fn extract_type_ref(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    references: &mut Vec<ReferenceEntry>,
) {
    let name = match node.kind() {
        "type_identifier" => node_text(node, source),
        "struct_specifier" | "union_specifier" | "enum_specifier" => {
            find_child_by_field(node, "name")
                .map(|n| node_text(n, source))
                .unwrap_or_default()
        }
        _ => return,
    };

    if name.is_empty() {
        return;
    }

    // Skip primitive types (they are in C_STOPWORDS but we check explicitly)
    if matches!(
        name.as_str(),
        "int" | "char" | "short" | "long" | "float" | "double" | "void" | "signed" | "unsigned"
    ) {
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

fn extract_declaration(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
) {
    // Top-level declarations: variables, function prototypes, extern declarations
    // Skip if inside a function body (we only want top-level)
    if let Some(p) = node.parent()
        && (p.kind() == "compound_statement" || p.kind() == "case_statement")
    {
        return;
    }

    let line = node_line_range(node);
    let is_static = has_storage_class(node, source, "static");
    let _is_extern = has_storage_class(node, source, "extern");
    let visibility = if is_static { "private" } else { "public" };

    // Walk declarators
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_declarator" => {
                // Function prototype
                let name = extract_declarator_name(child, source);
                if !name.is_empty() {
                    let _sig = collapse_whitespace(node_text(node, source).trim());
                    let kind = "function";
                    // Prototypes don't have a body, so no tokens
                    push_symbol(
                        symbols,
                        file_path,
                        name,
                        kind,
                        line,
                        parent_ctx,
                        None,
                        None,
                        Some(visibility.to_string()),
                    );
                }
            }
            "init_declarator" => {
                if let Some(decl) = find_child_by_field(child, "declarator") {
                    let name = extract_declarator_name(decl, source);
                    if !name.is_empty() {
                        let kind = if name.chars().all(|c| c.is_uppercase() || c == '_')
                            && name.len() > 1
                        {
                            "constant"
                        } else {
                            "variable"
                        };
                        push_symbol(
                            symbols,
                            file_path,
                            name,
                            kind,
                            line,
                            parent_ctx,
                            None,
                            None,
                            Some(visibility.to_string()),
                        );
                    }
                }
            }
            "identifier" | "pointer_declarator" => {
                let name = extract_declarator_name(child, source);
                if !name.is_empty() {
                    push_symbol(
                        symbols,
                        file_path,
                        name,
                        "variable",
                        line,
                        parent_ctx,
                        None,
                        None,
                        Some(visibility.to_string()),
                    );
                }
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_struct_or_union(
    node: Node,
    source: &[u8],
    file_path: &str,
    kind: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
    let name = find_child_by_field(node, "name")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    if name.is_empty() {
        // Anonymous struct/union, skip symbol but recurse for comments
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "comment" {
                extract_comment(child, source, file_path, parent_ctx, texts);
            }
        }
        return;
    }

    let line = node_line_range(node);

    push_symbol(
        symbols,
        file_path,
        name.clone(),
        kind,
        line,
        parent_ctx,
        None,
        None,
        Some("public".to_string()),
    );

    // Extract fields and their type references
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "field_declaration" {
                // Extract field type reference
                if let Some(type_node) = find_child_by_field(child, "type") {
                    extract_type_ref(type_node, source, file_path, Some(&name), references);
                }

                let mut field_cursor = child.walk();
                for field_child in child.children(&mut field_cursor) {
                    if field_child.kind() == "field_identifier" {
                        let field_name = node_text(field_child, source);
                        let field_line = node_line_range(child);
                        push_symbol(
                            symbols,
                            file_path,
                            format!("{name}.{field_name}"),
                            "property",
                            field_line,
                            Some(&name),
                            None,
                            None,
                            Some("public".to_string()),
                        );
                    }
                }
            }
            if child.kind() == "comment" {
                extract_comment(child, source, file_path, Some(&name), texts);
            }
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

    push_symbol(
        symbols,
        file_path,
        name.clone(),
        "enum",
        line,
        parent_ctx,
        None,
        None,
        Some("public".to_string()),
    );

    // Extract enum constants
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
                    format!("{name}.{const_name}"),
                    "constant",
                    const_line,
                    Some(&name),
                    None,
                    None,
                    Some("public".to_string()),
                );
            }
        }
    }
}

fn extract_typedef(node: Node, source: &[u8], file_path: &str, symbols: &mut Vec<SymbolEntry>) {
    let line = node_line_range(node);

    // The typedef name is typically the last declarator
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "type_identifier" || child.kind() == "identifier" {
            let name = node_text(child, source);
            push_symbol(
                symbols,
                file_path,
                name,
                "type_alias",
                line,
                None,
                None,
                None,
                Some("public".to_string()),
            );
        }
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
        // Strip < > or " "
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
        "identifier" => node_text(node, source),
        "field_identifier" => node_text(node, source),
        "pointer_declarator" => {
            // *name — recurse into the declarator
            find_child_by_field(node, "declarator")
                .map(|d| extract_declarator_name(d, source))
                .unwrap_or_default()
        }
        "function_declarator" => find_child_by_field(node, "declarator")
            .map(|d| extract_declarator_name(d, source))
            .unwrap_or_default(),
        "array_declarator" => find_child_by_field(node, "declarator")
            .map(|d| extract_declarator_name(d, source))
            .unwrap_or_default(),
        "parenthesized_declarator" => {
            // (name) — look inside
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                let name = extract_declarator_name(child, source);
                if !name.is_empty() {
                    return name;
                }
            }
            String::new()
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
    fn test_c_functions() {
        let source = b"int add(int a, int b) {
    return a + b;
}

static void helper() {
    printf(\"helper\");
}";
        let (symbols, _texts, _refs) = parse_file(source, "c", "test.c").unwrap();

        let add = find_sym(&symbols, "add");
        assert_eq!(add.kind, "function");
        // Token extraction is enabled (may be None if body has no tokens after filtering)
        assert_eq!(add.visibility.as_deref(), Some("public"));

        let helper = find_sym(&symbols, "helper");
        assert_eq!(helper.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_c_struct() {
        let source = b"struct Point {
    int x;
    int y;
};";
        let (symbols, _texts, _refs) = parse_file(source, "c", "test.c").unwrap();

        let point = find_sym(&symbols, "Point");
        assert_eq!(point.kind, "struct");

        let x = find_sym(&symbols, "Point.x");
        assert_eq!(x.kind, "property");
        assert_eq!(x.parent.as_deref(), Some("Point"));

        let y = find_sym(&symbols, "Point.y");
        assert_eq!(y.kind, "property");
    }

    #[test]
    fn test_c_enum() {
        let source = b"enum Status {
    OK,
    ERROR,
    PENDING
};";
        let (symbols, _texts, _refs) = parse_file(source, "c", "test.c").unwrap();

        let status = find_sym(&symbols, "Status");
        assert_eq!(status.kind, "enum");

        let ok = find_sym(&symbols, "Status.OK");
        assert_eq!(ok.kind, "constant");
        assert_eq!(ok.parent.as_deref(), Some("Status"));

        let error = find_sym(&symbols, "Status.ERROR");
        assert_eq!(error.kind, "constant");
    }

    #[test]
    fn test_c_typedef() {
        let source = b"typedef int MyInt;

int add(MyInt a, MyInt b) {
    return a + b;
}";
        let (symbols, _texts, _refs) = parse_file(source, "c", "test.c").unwrap();

        // Function should definitely be extracted
        let add = find_sym(&symbols, "add");
        assert_eq!(add.kind, "function");
    }

    #[test]
    fn test_c_variables() {
        let source = b"int global = 100;
static int file_scoped = 200;
extern int external;

#define MAX_SIZE 1000";
        let (symbols, _texts, _refs) = parse_file(source, "c", "test.c").unwrap();

        let global = find_sym(&symbols, "global");
        assert_eq!(global.kind, "variable");
        assert_eq!(global.visibility.as_deref(), Some("public"));

        let file_scoped = find_sym(&symbols, "file_scoped");
        assert_eq!(file_scoped.visibility.as_deref(), Some("private"));

        let max_size = find_sym(&symbols, "MAX_SIZE");
        assert_eq!(max_size.kind, "constant");
    }

    #[test]
    fn test_c_includes() {
        let source = b"#include <stdio.h>
#include \"myheader.h\"";
        let (symbols, _texts, _refs) = parse_file(source, "c", "test.c").unwrap();

        let stdio = symbols.iter().find(|s| s.name == "stdio.h").unwrap();
        assert_eq!(stdio.kind, "import");

        let myheader = symbols.iter().find(|s| s.name == "myheader.h").unwrap();
        assert_eq!(myheader.kind, "import");
    }

    #[test]
    fn test_c_macros() {
        let source = b"#define PI 3.14159
#define MAX(a, b) ((a) > (b) ? (a) : (b))";
        let (symbols, _texts, _refs) = parse_file(source, "c", "test.c").unwrap();

        let pi = find_sym(&symbols, "PI");
        assert_eq!(pi.kind, "constant");

        let max = find_sym(&symbols, "MAX");
        assert_eq!(max.kind, "macro");
    }

    #[test]
    fn test_c_union() {
        let source = b"union Data {
    int i;
    float f;
    char str[20];
};";
        let (symbols, _texts, _refs) = parse_file(source, "c", "test.c").unwrap();

        let data = find_sym(&symbols, "Data");
        assert_eq!(data.kind, "struct"); // unions mapped to struct

        let i = find_sym(&symbols, "Data.i");
        assert_eq!(i.kind, "property");
    }

    #[test]
    fn test_c_comments() {
        let source = b"/* Block comment */
// Single line comment
int foo() { return 0; }";
        let (_symbols, texts, _refs) = parse_file(source, "c", "test.c").unwrap();
        assert!(texts.iter().any(|t| t.kind == "comment"));
    }

    #[test]
    fn test_c_function_prototype() {
        let source = b"int add(int a, int b);
extern void print(const char* msg);";
        let (symbols, _texts, _refs) = parse_file(source, "c", "test.c").unwrap();

        let add = find_sym(&symbols, "add");
        assert_eq!(add.kind, "function");
        // Prototypes don't have bodies, so no tokens
        assert!(add.tokens.is_none());
    }

    #[test]
    fn test_c_call_references() {
        let source = b"void foo() {}
void bar() {
    foo();
    my_custom_function();
}";
        let (_symbols, _texts, refs) = parse_file(source, "c", "test.c").unwrap();

        let calls: Vec<_> = refs.iter().filter(|r| r.kind == "call").collect();
        assert!(calls.iter().any(|r| r.name == "foo"));
        assert!(calls.iter().any(|r| r.name == "my_custom_function"));
    }

    #[test]
    fn test_c_include_references() {
        let source = b"#include <stdio.h>
#include \"myheader.h\"";
        let (_symbols, _texts, refs) = parse_file(source, "c", "test.c").unwrap();

        let imports: Vec<_> = refs.iter().filter(|r| r.kind == "import").collect();
        assert!(imports.iter().any(|r| r.name == "stdio.h"));
        assert!(imports.iter().any(|r| r.name == "myheader.h"));
    }

    #[test]
    fn test_c_type_references() {
        let source = b"typedef struct MyStruct MyStruct;

MyStruct* create_mystruct() {
    return NULL;
}

struct Container {
    MyStruct* data;
};";
        let (_symbols, _texts, refs) = parse_file(source, "c", "test.c").unwrap();

        let type_refs: Vec<_> = refs
            .iter()
            .filter(|r| r.kind == "type_annotation")
            .collect();
        assert!(type_refs.iter().any(|r| r.name == "MyStruct"));
    }
}
