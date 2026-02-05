//! C# symbol and text extraction.

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
        "class_declaration" => {
            extract_type_decl(node, source, file_path, "class", parent_ctx, symbols, texts);
            return;
        }
        "struct_declaration" => {
            extract_type_decl(
                node, source, file_path, "struct", parent_ctx, symbols, texts,
            );
            return;
        }
        "interface_declaration" => {
            extract_type_decl(
                node,
                source,
                file_path,
                "interface",
                parent_ctx,
                symbols,
                texts,
            );
            return;
        }
        "enum_declaration" => {
            extract_enum(node, source, file_path, parent_ctx, symbols);
        }
        "record_declaration" => {
            extract_type_decl(
                node, source, file_path, "struct", parent_ctx, symbols, texts,
            );
            return;
        }
        "namespace_declaration" | "file_scoped_namespace_declaration" => {
            extract_namespace(node, source, file_path, parent_ctx, symbols, texts);
            return;
        }
        "method_declaration" => {
            extract_method(node, source, file_path, parent_ctx, symbols);
        }
        "constructor_declaration" => {
            extract_constructor(node, source, file_path, parent_ctx, symbols);
        }
        "property_declaration" => {
            extract_property(node, source, file_path, parent_ctx, symbols);
        }
        "field_declaration" => {
            extract_field(node, source, file_path, parent_ctx, symbols);
        }
        "delegate_declaration" => {
            extract_delegate(node, source, file_path, parent_ctx, symbols);
        }
        "using_directive" => {
            extract_using(node, source, file_path, symbols);
        }
        "comment" => {
            extract_csharp_comment(node, source, file_path, parent_ctx, texts);
            return;
        }
        "string_literal"
        | "verbatim_string_literal"
        | "interpolated_string_expression"
        | "raw_string_literal" => {
            extract_string(node, source, file_path, parent_ctx, texts);
            return;
        }
        _ => {}
    }

    // Recurse
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_node(child, source, file_path, parent_ctx, symbols, texts);
    }
}

fn extract_type_decl(
    node: Node,
    source: &[u8],
    file_path: &str,
    kind: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    texts: &mut Vec<TextEntry>,
) {
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let line = node_line_range(node);
    let visibility = extract_csharp_visibility(node, source);

    let type_params = find_child_by_field(node, "type_parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    let bases = find_child_by_field(node, "bases")
        .or_else(|| {
            let mut cursor = node.walk();
            node.children(&mut cursor).find(|c| c.kind() == "base_list")
        })
        .map(|n| format!(" : {}", node_text(n, source)))
        .unwrap_or_default();

    let sig = format!("{kind} {name}{type_params}{bases}");

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

    // Walk body
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            walk_node(child, source, file_path, Some(&full_name), symbols, texts);
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
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let line = node_line_range(node);
    let visibility = extract_csharp_visibility(node, source);

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
        Some(visibility),
    );

    // Extract enum members
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "enum_member_declaration" {
                if let Some(name_node) = find_child_by_field(child, "name") {
                    let member_name = node_text(name_node, source);
                    let member_line = node_line_range(child);
                    push_symbol(
                        symbols,
                        file_path,
                        format!("{full_name}.{member_name}"),
                        "constant",
                        member_line,
                        Some(&full_name),
                        None,
                        None,
                        Some("public".to_string()),
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
    let name = match find_child_by_field(node, "name") {
        Some(n) => node_text(n, source),
        None => return,
    };

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

    // Walk namespace body
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            walk_node(child, source, file_path, Some(&full_name), symbols, texts);
        }
    }

    // File-scoped namespace: declarations are siblings, not children of body
    if node.kind() == "file_scoped_namespace_declaration" {
        // Walk all siblings after this node
        if let Some(parent) = node.parent() {
            let mut found_ns = false;
            let mut cursor = parent.walk();
            for child in parent.children(&mut cursor) {
                if child.id() == node.id() {
                    found_ns = true;
                    continue;
                }
                if found_ns {
                    walk_node(child, source, file_path, Some(&full_name), symbols, texts);
                }
            }
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
    let visibility = extract_csharp_visibility(node, source);
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
    let visibility = extract_csharp_visibility(node, source);
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

fn extract_property(
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
    let visibility = extract_csharp_visibility(node, source);

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name
    };

    push_symbol(
        symbols,
        file_path,
        full_name,
        "property",
        line,
        parent_ctx,
        None,
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
    let visibility = extract_csharp_visibility(node, source);

    let is_const = has_csharp_modifier(node, source, "const");
    let is_readonly = has_csharp_modifier(node, source, "readonly");
    let is_static = has_csharp_modifier(node, source, "static");

    let kind = if is_const || (is_static && is_readonly) {
        "constant"
    } else {
        "property"
    };

    // Walk variable declarators
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declaration" {
            let mut decl_cursor = child.walk();
            for decl_child in child.children(&mut decl_cursor) {
                if decl_child.kind() == "variable_declarator" {
                    if let Some(name_node) = find_child_by_field(decl_child, "name").or_else(|| {
                        let mut c = decl_child.walk();
                        decl_child
                            .children(&mut c)
                            .find(|n| n.kind() == "identifier")
                    }) {
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
        }
    }
}

fn extract_delegate(
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
    let visibility = extract_csharp_visibility(node, source);
    let sig = collapse_whitespace(node_text(node, source).trim());

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
        Some(visibility),
    );
}

fn extract_using(node: Node, source: &[u8], file_path: &str, symbols: &mut Vec<SymbolEntry>) {
    let line = node_line_range(node);

    // `using System.Linq;` or `using Foo = System.Bar;`
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "qualified_name" | "identifier" | "identifier_name" => {
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
            "name_equals" => {
                // `using Alias = Namespace.Type;`
                let alias = find_child_by_field(child, "name").map(|n| node_text(n, source));
                if let Some(a) = alias {
                    // The actual type is the next sibling
                    let type_name = child
                        .next_sibling()
                        .map(|n| node_text(n, source))
                        .unwrap_or_default();
                    if !type_name.is_empty() {
                        push_symbol(
                            symbols,
                            file_path,
                            type_name,
                            "import",
                            line,
                            None,
                            None,
                            Some(a),
                            Some("private".to_string()),
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn extract_csharp_comment(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    let raw = node_text(node, source);
    let line = node_line_range(node);

    let (kind, text) = if raw.starts_with("///") {
        // XML doc comment
        let cleaned = raw
            .lines()
            .map(|l| {
                let t = l.trim();
                t.strip_prefix("///").unwrap_or(t).trim()
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        ("docstring", cleaned)
    } else if raw.starts_with("/**") {
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

fn extract_csharp_visibility(node: Node, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifier" {
            let text = node_text(child, source);
            match text.as_str() {
                "public" => return "public".to_string(),
                "private" => return "private".to_string(),
                "protected" | "internal" => return "internal".to_string(),
                _ => {}
            }
        }
    }
    // Check for modifiers list pattern (some tree-sitter versions)
    let full_text = node_text(node, source);
    if full_text.starts_with("public ") {
        return "public".to_string();
    }
    if full_text.starts_with("private ") {
        return "private".to_string();
    }
    if full_text.starts_with("protected ") || full_text.starts_with("internal ") {
        return "internal".to_string();
    }
    "private".to_string() // C# default
}

fn has_csharp_modifier(node: Node, source: &[u8], modifier: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifier" {
            let text = node_text(child, source);
            if text == modifier {
                return true;
            }
        }
    }
    // Fallback: check the raw text
    let text = node_text(node, source);
    text.contains(&format!("{modifier} "))
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
    fn test_csharp_class() {
        let source = b"public class Person
{
    private string name;

    public Person(string name)
    {
        this.name = name;
    }

    public string GetName()
    {
        return name;
    }

    private void Helper() {}
}";
        let (symbols, _texts) = parse_file(source, "csharp", "test.cs").unwrap();

        let person = find_sym(&symbols, "Person");
        assert_eq!(person.kind, "class");
        assert_eq!(person.visibility.as_deref(), Some("public"));
        assert!(person.sig.as_ref().unwrap().contains("class Person"));

        let name = find_sym(&symbols, "Person.name");
        assert_eq!(name.kind, "property");
        assert_eq!(name.visibility.as_deref(), Some("private"));

        let get_name = find_sym(&symbols, "Person.GetName");
        assert_eq!(get_name.kind, "method");
        assert_eq!(get_name.visibility.as_deref(), Some("public"));

        let helper = find_sym(&symbols, "Person.Helper");
        assert_eq!(helper.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_csharp_interface() {
        let source = b"public interface IRunnable
{
    void Run();
    int Calculate(int x);
}";
        let (symbols, _texts) = parse_file(source, "csharp", "test.cs").unwrap();

        let runnable = find_sym(&symbols, "IRunnable");
        assert_eq!(runnable.kind, "interface");
        assert_eq!(runnable.visibility.as_deref(), Some("public"));
    }

    #[test]
    fn test_csharp_struct() {
        let source = b"public struct Point
{
    public int X;
    public int Y;
}";
        let (symbols, _texts) = parse_file(source, "csharp", "test.cs").unwrap();

        let point = find_sym(&symbols, "Point");
        assert_eq!(point.kind, "struct");

        let x = find_sym(&symbols, "Point.X");
        assert_eq!(x.kind, "property");
        assert_eq!(x.visibility.as_deref(), Some("public"));
    }

    #[test]
    fn test_csharp_enum() {
        let source = b"public enum Status
{
    Active,
    Inactive,
    Pending
}";
        let (symbols, _texts) = parse_file(source, "csharp", "test.cs").unwrap();

        let status = find_sym(&symbols, "Status");
        assert_eq!(status.kind, "enum");

        let active = find_sym(&symbols, "Status.Active");
        assert_eq!(active.kind, "constant");
        assert_eq!(active.parent.as_deref(), Some("Status"));
    }

    #[test]
    fn test_csharp_namespace() {
        let source = b"namespace MyApp.Utils
{
    public class Helper
    {
        public void Run() {}
    }
}";
        let (symbols, _texts) = parse_file(source, "csharp", "test.cs").unwrap();

        let utils = find_sym(&symbols, "MyApp.Utils");
        assert_eq!(utils.kind, "module");

        let helper = find_sym(&symbols, "MyApp.Utils.Helper");
        assert_eq!(helper.kind, "class");
        assert_eq!(helper.parent.as_deref(), Some("MyApp.Utils"));

        let run = find_sym(&symbols, "MyApp.Utils.Helper.Run");
        assert_eq!(run.kind, "method");
    }

    #[test]
    fn test_csharp_properties() {
        let source = b"public class Config
{
    public string Name { get; set; }
    private int version;
}";
        let (symbols, _texts) = parse_file(source, "csharp", "test.cs").unwrap();

        let name = find_sym(&symbols, "Config.Name");
        assert_eq!(name.kind, "property");
        assert_eq!(name.visibility.as_deref(), Some("public"));

        let version = find_sym(&symbols, "Config.version");
        assert_eq!(version.kind, "property");
        assert_eq!(version.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_csharp_constants() {
        let source = b"public class Constants
{
    public const int MAX_SIZE = 100;
    private static readonly string Version = \"1.0\";
}";
        let (symbols, _texts) = parse_file(source, "csharp", "test.cs").unwrap();

        let max = find_sym(&symbols, "Constants.MAX_SIZE");
        assert_eq!(max.kind, "constant");
        assert_eq!(max.visibility.as_deref(), Some("public"));

        let version = find_sym(&symbols, "Constants.Version");
        assert_eq!(version.kind, "constant");
        assert_eq!(version.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_csharp_using() {
        let source = b"using System;
using System.Collections.Generic;
using System.Linq;";
        let (symbols, _texts) = parse_file(source, "csharp", "test.cs").unwrap();

        let system = symbols.iter().find(|s| s.name == "System").unwrap();
        assert_eq!(system.kind, "import");

        let generic = symbols
            .iter()
            .find(|s| s.name == "System.Collections.Generic")
            .unwrap();
        assert_eq!(generic.kind, "import");

        let linq = symbols.iter().find(|s| s.name == "System.Linq").unwrap();
        assert_eq!(linq.kind, "import");
    }

    #[test]
    fn test_csharp_delegate() {
        let source = b"public delegate void EventHandler(object sender);";
        let (symbols, _texts) = parse_file(source, "csharp", "test.cs").unwrap();

        let handler = find_sym(&symbols, "EventHandler");
        assert_eq!(handler.kind, "type_alias");
        assert_eq!(handler.visibility.as_deref(), Some("public"));
    }

    #[test]
    fn test_csharp_visibility() {
        let source = b"public class Foo
{
    public void PublicMethod() {}
    private void PrivateMethod() {}
    protected void ProtectedMethod() {}
    internal void InternalMethod() {}
}";
        let (symbols, _texts) = parse_file(source, "csharp", "test.cs").unwrap();

        let public = find_sym(&symbols, "Foo.PublicMethod");
        assert_eq!(public.visibility.as_deref(), Some("public"));

        let private = find_sym(&symbols, "Foo.PrivateMethod");
        assert_eq!(private.visibility.as_deref(), Some("private"));

        let protected = find_sym(&symbols, "Foo.ProtectedMethod");
        assert_eq!(protected.visibility.as_deref(), Some("internal"));

        let internal = find_sym(&symbols, "Foo.InternalMethod");
        assert_eq!(internal.visibility.as_deref(), Some("internal"));
    }

    #[test]
    fn test_csharp_comments() {
        let source = b"/// <summary>
/// XML doc comment
/// </summary>
public class Documented {}

// Single line
/* Block comment */";
        let (_symbols, texts) = parse_file(source, "csharp", "test.cs").unwrap();
        assert!(
            texts
                .iter()
                .any(|t| t.kind == "docstring" || t.kind == "comment")
        );
    }
}
