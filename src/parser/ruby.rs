//! Ruby symbol and text extraction.

use tree_sitter::{Node, Tree};

use crate::index::format::{ReferenceEntry, SymbolEntry, TextEntry};
use crate::parser::helpers::*;
use crate::parser::treesitter::MAX_DEPTH;

/// Ruby-specific stopwords (keywords, common patterns)
const RUBY_STOPWORDS: &[&str] = &[
    // Keywords
    "def",
    "end",
    "module",
    "elsif",
    "unless",
    "when",
    "until",
    "begin",
    "rescue",
    "ensure",
    "raise",
    "yield",
    "next",
    "redo",
    "retry",
    "self",
    "nil",
    "and",
    "or",
    "not",
    "then",
    "alias",
    "defined",
    "undef",
    // Common patterns
    "attr",
    "attr_reader",
    "attr_writer",
    "attr_accessor",
    "include",
    "extend",
    "require",
    "require_relative",
    "initialize",
    "puts",
    "print",
    "gets",
    "p",
    // Common variable patterns
    "args",
    "opts",
    "options",
    "block",
    "proc",
    "lambda",
    // Very common methods (too generic)
    "each",
    "map",
    "select",
    "reject",
    "find",
    "first",
    "last",
    "length",
    "size",
    "empty",
    "to_s",
    "to_i",
    "to_a",
    "to_h",
    "join",
    "split",
    "fetch",
    "sample",
];

/// Filter Ruby-specific stopwords from extracted tokens.
fn filter_ruby_tokens(tokens: Option<String>) -> Option<String> {
    tokens.and_then(|t| {
        let filtered: Vec<&str> = t
            .split_whitespace()
            .filter(|tok| !RUBY_STOPWORDS.contains(&tok.to_lowercase().as_str()))
            // Filter uppercase constants (ALL_CAPS)
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

/// Check if a method name is a Ruby builtin or common library method.
fn is_ruby_builtin_call(name: &str) -> bool {
    matches!(
        name,
        // Kernel methods
        "puts"
        | "print"
        | "p"
        | "pp"
        | "gets"
        | "require"
        | "require_relative"
        | "load"
        | "raise"
        | "fail"
        | "catch"
        | "throw"
        | "exit"
        | "abort"
        | "sleep"
        | "rand"
        | "srand"
        | "loop"
        | "lambda"
        | "proc"
        | "binding"
        | "eval"
        | "exec"
        | "system"
        | "spawn"
        | "fork"
        | "open"
        | "sprintf"
        | "format"
        | "warn"
        // Object methods
        | "class"
        | "object_id"
        | "send"
        | "public_send"
        | "respond_to?"
        | "method"
        | "methods"
        | "instance_variables"
        | "is_a?"
        | "kind_of?"
        | "instance_of?"
        | "nil?"
        | "tap"
        | "then"
        | "yield_self"
        | "freeze"
        | "frozen?"
        | "dup"
        | "clone"
        | "to_s"
        | "to_a"
        | "to_h"
        | "to_i"
        | "to_f"
        | "to_sym"
        | "inspect"
        | "hash"
        | "eql?"
        | "equal?"
        // Array/Enumerable methods
        | "each"
        | "each_with_index"
        | "each_with_object"
        | "map"
        | "collect"
        | "select"
        | "filter"
        | "reject"
        | "find"
        | "detect"
        | "find_all"
        | "first"
        | "last"
        | "take"
        | "drop"
        | "count"
        | "length"
        | "size"
        | "empty?"
        | "any?"
        | "all?"
        | "none?"
        | "one?"
        | "include?"
        | "member?"
        | "min"
        | "max"
        | "minmax"
        | "sum"
        | "reduce"
        | "inject"
        | "sort"
        | "sort_by"
        | "reverse"
        | "flatten"
        | "compact"
        | "uniq"
        | "zip"
        | "group_by"
        | "partition"
        | "push"
        | "pop"
        | "shift"
        | "unshift"
        | "delete"
        | "clear"
        | "concat"
        | "join"
        | "split"
        // Hash methods
        | "keys"
        | "values"
        | "fetch"
        | "store"
        | "merge"
        | "merge!"
        | "update"
        | "has_key?"
        | "key?"
        | "has_value?"
        | "value?"
        | "dig"
        // String methods
        | "strip"
        | "chomp"
        | "chop"
        | "upcase"
        | "downcase"
        | "capitalize"
        | "gsub"
        | "sub"
        | "match"
        | "match?"
        | "start_with?"
        | "end_with?"
        | "encode"
        | "bytes"
        | "chars"
        | "lines"
        // Attr accessors
        | "attr_reader"
        | "attr_writer"
        | "attr_accessor"
        | "attr"
        // Module/Class methods
        | "include"
        | "extend"
        | "prepend"
        | "alias_method"
        | "define_method"
        | "module_function"
        | "private"
        | "protected"
        | "public"
        | "new"
        | "initialize"
        | "allocate"
        | "superclass"
        | "ancestors"
        // Test framework
        | "describe"
        | "context"
        | "it"
        | "before"
        | "after"
        | "let"
        | "expect"
        | "should"
        | "assert"
        | "assert_equal"
        | "refute"
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
        "method" => {
            extract_method(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return;
        }
        "singleton_method" => {
            extract_singleton_method(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return;
        }
        "class" => {
            extract_class(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return;
        }
        "module" => {
            extract_module(
                node, source, file_path, parent_ctx, symbols, texts, references, depth,
            );
            return;
        }
        "assignment" => {
            extract_assignment(node, source, file_path, parent_ctx, symbols);
        }
        "call" | "method_call" => {
            // Capture `require`, `include`, `extend`, `attr_*`, and method calls
            extract_call(node, source, file_path, parent_ctx, symbols, references);
        }
        "comment" => {
            extract_ruby_comment(node, source, file_path, parent_ctx, texts);
            return;
        }
        "string" | "heredoc_body" | "string_content" => {
            if kind == "string" {
                extract_string(node, source, file_path, parent_ctx, texts);
            }
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
    let visibility = ruby_visibility(&name);

    let params = find_child_by_field(node, "parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    let _sig = format!("def {name}{params}");

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

    // Extract tokens from method body
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| filter_ruby_tokens(extract_tokens(body, source)));

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

    // Recurse for nested definitions and call references
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            // Walk all children to find calls and nested definitions
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
fn extract_singleton_method(
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

    let params = find_child_by_field(node, "parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    let _sig = format!("def self.{name}{params}");

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
    };

    // Extract tokens from singleton method body
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| filter_ruby_tokens(extract_tokens(body, source)));

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        "method",
        line,
        parent_ctx,
        tokens,
        None,
        Some("public".to_string()),
    );

    // Recurse for nested definitions and call references
    if let Some(body) = find_child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            // Walk all children to find calls and nested definitions
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

    // Check for superclass
    let superclass = find_child_by_field(node, "superclass");
    let superclass_str = superclass
        .map(|n| format!(" < {}", node_text(n, source)))
        .unwrap_or_default();

    let _sig = format!("class {name}{superclass_str}");

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
    };

    // Extract superclass reference (inheritance)
    if let Some(super_node) = superclass {
        // The superclass field may contain a "superclass" node
        // We need to find the constant child (the actual class name)
        let super_name = if super_node.kind() == "superclass" {
            // Find the constant or scope_resolution child (skip the "<" operator)
            let mut cursor = super_node.walk();
            super_node
                .children(&mut cursor)
                .find(|c| matches!(c.kind(), "constant" | "scope_resolution"))
                .map(|c| node_text(c, source))
                .unwrap_or_default()
        } else if matches!(super_node.kind(), "constant" | "scope_resolution") {
            node_text(super_node, source)
        } else {
            String::new()
        };
        if !super_name.is_empty() && !is_ruby_builtin_call(&super_name) {
            references.push(ReferenceEntry {
                file: file_path.to_string(),
                name: super_name,
                kind: "type_annotation".to_string(),
                line: node_line_range(super_node),
                caller: Some(full_name.clone()),
                project: String::new(),
            });
        }
    }

    // Extract tokens from class body
    let tokens = find_child_by_field(node, "body")
        .and_then(|body| filter_ruby_tokens(extract_tokens(body, source)));

    push_symbol(
        symbols,
        file_path,
        full_name.clone(),
        "class",
        line,
        parent_ctx,
        tokens,
        None,
        Some("public".to_string()),
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

#[allow(clippy::too_many_arguments)]
fn extract_module(
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
                symbols,
                texts,
                references,
                depth + 1,
            );
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
    let left = match find_child_by_field(node, "left") {
        Some(n) => n,
        None => return,
    };

    let name = node_text(left, source);
    let line = node_line_range(node);

    match left.kind() {
        "constant" => {
            // CONSTANT = value
            let full_name = if let Some(parent) = parent_ctx {
                format!("{parent}.{name}")
            } else {
                name
            };
            push_symbol(
                symbols,
                file_path,
                full_name,
                "constant",
                line,
                parent_ctx,
                None,
                None,
                Some("public".to_string()),
            );
        }
        "identifier" => {
            // Module/class level assignment
            if parent_ctx.is_none() {
                // Only capture top-level assignments
                let visibility = ruby_visibility(&name);
                push_symbol(
                    symbols,
                    file_path,
                    name,
                    "variable",
                    line,
                    parent_ctx,
                    None,
                    None,
                    Some(visibility),
                );
            }
        }
        "instance_variable" | "class_variable" => {
            // @var or @@var
            let visibility = if name.starts_with("@@") {
                "internal".to_string()
            } else {
                "private".to_string()
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
                "property",
                line,
                parent_ctx,
                None,
                None,
                Some(visibility),
            );
        }
        _ => {}
    }
}

fn extract_call(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
    references: &mut Vec<ReferenceEntry>,
) {
    let method = match find_child_by_field(node, "method") {
        Some(n) => node_text(n, source),
        None => return,
    };

    let line = node_line_range(node);

    match method.as_str() {
        "require" | "require_relative" | "load" => {
            // Extract the required file
            if let Some(args) = find_child_by_field(node, "arguments") {
                let mut cursor = args.walk();
                for child in args.children(&mut cursor) {
                    if child.kind() == "string" || child.kind() == "string_content" {
                        let path = strip_string_quotes(&node_text(child, source));
                        if !path.is_empty() {
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
                }
            }
        }
        _ => {
            // Extract other method calls as references
            if !is_ruby_builtin_call(&method) {
                // Build the full call name including receiver
                let call_name = if let Some(receiver) = find_child_by_field(node, "receiver") {
                    let receiver_name = node_text(receiver, source);
                    if receiver_name.is_empty() || receiver_name == "self" {
                        method.clone()
                    } else {
                        format!("{}.{}", receiver_name, method)
                    }
                } else {
                    method
                };

                if !call_name.is_empty() {
                    references.push(ReferenceEntry {
                        file: file_path.to_string(),
                        name: call_name,
                        kind: "call".to_string(),
                        line,
                        caller: parent_ctx.map(String::from),
                        project: String::new(),
                    });
                }
            }
        }
    }
}

fn extract_ruby_comment(
    node: Node,
    source: &[u8],
    file_path: &str,
    parent_ctx: Option<&str>,
    texts: &mut Vec<TextEntry>,
) {
    extract_comment(node, source, file_path, parent_ctx, texts);
}

fn ruby_visibility(name: &str) -> String {
    if name.starts_with('_') {
        "private".to_string()
    } else {
        "public".to_string()
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
    fn test_ruby_methods() {
        let source = b"def hello(name)
  \"Hello, #{name}!\"
end

def _private_helper
  puts 'private'
end";
        let (symbols, _texts, _refs) = parse_file(source, "ruby", "test.rb").unwrap();

        let hello = find_sym(&symbols, "hello");
        assert_eq!(hello.kind, "function");
        // Token extraction extracts identifiers from method body
        // Token may be None if all identifiers are filtered as stopwords
        assert_eq!(hello.visibility.as_deref(), Some("public"));

        let helper = find_sym(&symbols, "_private_helper");
        assert_eq!(helper.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_ruby_classes() {
        let source = b"class Person
  def initialize(name)
    @name = name
  end

  def greet
    \"Hi, #{@name}\"
  end

  def self.create
    Person.new('default')
  end
end";
        let (symbols, _texts, _refs) = parse_file(source, "ruby", "test.rb").unwrap();

        let person = find_sym(&symbols, "Person");
        assert_eq!(person.kind, "class");

        let init = find_sym(&symbols, "Person.initialize");
        assert_eq!(init.kind, "method");
        assert_eq!(init.parent.as_deref(), Some("Person"));

        let create = find_sym(&symbols, "Person.create");
        assert_eq!(create.kind, "method");
    }

    #[test]
    fn test_ruby_modules() {
        let source = b"module Utils
  def self.helper
    puts 'helping'
  end
end

module Logger
  class Writer
  end
end";
        let (symbols, _texts, _refs) = parse_file(source, "ruby", "test.rb").unwrap();

        let utils = find_sym(&symbols, "Utils");
        assert_eq!(utils.kind, "module");

        let helper = find_sym(&symbols, "Utils.helper");
        assert_eq!(helper.parent.as_deref(), Some("Utils"));

        let logger = find_sym(&symbols, "Logger");
        assert_eq!(logger.kind, "module");

        let writer = find_sym(&symbols, "Logger.Writer");
        assert_eq!(writer.kind, "class");
        assert_eq!(writer.parent.as_deref(), Some("Logger"));
    }

    #[test]
    fn test_ruby_constants() {
        let source = b"MAX_SIZE = 100
DEFAULT_NAME = 'Unknown'

class Config
  VERSION = '1.0'
end";
        let (symbols, _texts, _refs) = parse_file(source, "ruby", "test.rb").unwrap();

        let max = find_sym(&symbols, "MAX_SIZE");
        assert_eq!(max.kind, "constant");

        let version = find_sym(&symbols, "Config.VERSION");
        assert_eq!(version.kind, "constant");
        assert_eq!(version.parent.as_deref(), Some("Config"));
    }

    #[test]
    fn test_ruby_variables() {
        let source = b"class Foo
  @instance_var = 1
  @@class_var = 2
end";
        let (symbols, _texts, _refs) = parse_file(source, "ruby", "test.rb").unwrap();

        let instance = find_sym(&symbols, "Foo.@instance_var");
        assert_eq!(instance.kind, "property");
        assert_eq!(instance.visibility.as_deref(), Some("private"));

        let class_var = find_sym(&symbols, "Foo.@@class_var");
        assert_eq!(class_var.kind, "property");
        assert_eq!(class_var.visibility.as_deref(), Some("internal"));
    }

    #[test]
    fn test_ruby_require() {
        let source = b"require 'json'
require_relative 'config'
require 'active_support'";
        let (symbols, _texts, _refs) = parse_file(source, "ruby", "test.rb").unwrap();

        let json = symbols.iter().find(|s| s.name == "json").unwrap();
        assert_eq!(json.kind, "import");

        let config = symbols.iter().find(|s| s.name == "config").unwrap();
        assert_eq!(config.kind, "import");
    }

    #[test]
    fn test_ruby_inheritance() {
        let source = b"class Animal
end

class Dog < Animal
  def bark
    'woof'
  end
end";
        let (symbols, _texts, _refs) = parse_file(source, "ruby", "test.rb").unwrap();

        let dog = find_sym(&symbols, "Dog");
        assert_eq!(dog.kind, "class");

        let bark = find_sym(&symbols, "Dog.bark");
        assert_eq!(bark.kind, "method");
    }

    #[test]
    fn test_ruby_singleton_methods() {
        let source = b"class Utils
  def self.format(text)
    text.upcase
  end
end";
        let (symbols, _texts, _refs) = parse_file(source, "ruby", "test.rb").unwrap();

        let format = find_sym(&symbols, "Utils.format");
        assert_eq!(format.kind, "method");
    }

    #[test]
    fn test_ruby_comments() {
        let source = b"# Single line comment
def foo
end

=begin
Multi-line comment
=end";
        let (_symbols, texts, _refs) = parse_file(source, "ruby", "test.rb").unwrap();
        assert!(texts.iter().any(|t| t.kind == "comment"));
    }

    #[test]
    fn test_ruby_visibility_markers() {
        let source = b"def public_method
end

def _internal_method
end";
        let (symbols, _texts, _refs) = parse_file(source, "ruby", "test.rb").unwrap();

        let public = find_sym(&symbols, "public_method");
        assert_eq!(public.visibility.as_deref(), Some("public"));

        let internal = find_sym(&symbols, "_internal_method");
        assert_eq!(internal.visibility.as_deref(), Some("private"));
    }

    #[test]
    fn test_ruby_call_references() {
        let source = b"class Foo
  def bar
    my_service.process
    helper_function()
  end
end";
        let (_symbols, _texts, refs) = parse_file(source, "ruby", "test.rb").unwrap();

        let calls: Vec<_> = refs.iter().filter(|r| r.kind == "call").collect();
        assert!(
            calls.iter().any(|r| r.name.contains("process")),
            "Expected to find 'process' in calls: {:?}",
            calls
        );
        assert!(
            calls.iter().any(|r| r.name == "helper_function"),
            "Expected to find 'helper_function' in calls: {:?}",
            calls
        );
    }

    #[test]
    fn test_ruby_require_references() {
        let source = b"require 'json'
require_relative 'config'
require 'my_custom_gem'";
        let (_symbols, _texts, refs) = parse_file(source, "ruby", "test.rb").unwrap();

        let imports: Vec<_> = refs.iter().filter(|r| r.kind == "import").collect();
        assert!(imports.iter().any(|r| r.name == "json"));
        assert!(imports.iter().any(|r| r.name == "config"));
        assert!(imports.iter().any(|r| r.name == "my_custom_gem"));
    }

    #[test]
    fn test_ruby_inheritance_references() {
        let source = b"class Animal
end

class Dog < Animal
  def bark
    'woof'
  end
end

class CustomWidget < BaseWidget
end";
        let (_symbols, _texts, refs) = parse_file(source, "ruby", "test.rb").unwrap();

        let type_refs: Vec<_> = refs
            .iter()
            .filter(|r| r.kind == "type_annotation")
            .collect();
        assert!(type_refs.iter().any(|r| r.name == "Animal"));
        assert!(type_refs.iter().any(|r| r.name == "BaseWidget"));
    }
}
