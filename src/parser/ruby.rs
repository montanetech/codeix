//! Ruby symbol and text extraction.

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
        "method" => {
            extract_method(node, source, file_path, parent_ctx, symbols, texts, depth);
            return;
        }
        "singleton_method" => {
            extract_singleton_method(node, source, file_path, parent_ctx, symbols, texts, depth);
            return;
        }
        "class" => {
            extract_class(node, source, file_path, parent_ctx, symbols, texts, depth);
            return;
        }
        "module" => {
            extract_module(node, source, file_path, parent_ctx, symbols, texts, depth);
            return;
        }
        "assignment" => {
            extract_assignment(node, source, file_path, parent_ctx, symbols);
        }
        "call" => {
            // Capture `require`, `include`, `extend`, `attr_*`
            extract_call(node, source, file_path, parent_ctx, symbols);
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
            depth + 1,
        );
    }
}

fn extract_method(
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

    push_symbol(
        symbols,
        file_path,
        full_name,
        kind,
        line,
        parent_ctx,
        None, // TODO: add token extraction
        None,
        Some(visibility),
    );

    // Recurse for nested definitions
    if let Some(body) = find_child_by_field(node, "body") {
        let ctx = if let Some(p) = parent_ctx {
            format!("{}.{}", p, name)
        } else {
            name
        };
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            match child.kind() {
                "method" | "singleton_method" | "class" | "module" => {
                    walk_node(
                        child,
                        source,
                        file_path,
                        Some(&ctx),
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

fn extract_singleton_method(
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

    let params = find_child_by_field(node, "parameters")
        .map(|n| node_text(n, source))
        .unwrap_or_default();

    let _sig = format!("def self.{name}{params}");

    let full_name = if let Some(parent) = parent_ctx {
        format!("{parent}.{name}")
    } else {
        name.clone()
    };

    push_symbol(
        symbols,
        file_path,
        full_name,
        "method",
        line,
        parent_ctx,
        None, // TODO: add token extraction
        None,
        Some("public".to_string()),
    );

    // Recurse for nested definitions
    if let Some(body) = find_child_by_field(node, "body") {
        let ctx = if let Some(p) = parent_ctx {
            format!("{}.{}", p, name)
        } else {
            name
        };
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            match child.kind() {
                "method" | "singleton_method" | "class" | "module" => {
                    walk_node(
                        child,
                        source,
                        file_path,
                        Some(&ctx),
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

    // Check for superclass
    let superclass = find_child_by_field(node, "superclass")
        .map(|n| format!(" < {}", node_text(n, source)))
        .unwrap_or_default();

    let _sig = format!("class {name}{superclass}");

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
        None, // TODO: add token extraction
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
                depth + 1,
            );
        }
    }
}

fn extract_module(
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
    _parent_ctx: Option<&str>,
    symbols: &mut Vec<SymbolEntry>,
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
                                path,
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
            }
        }
        _ => {}
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
        let (symbols, _texts) = parse_file(source, "ruby", "test.rb").unwrap();

        let hello = find_sym(&symbols, "hello");
        assert_eq!(hello.kind, "function");
        // Token extraction not yet implemented for Ruby
        assert!(hello.tokens.is_none());
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
        let (symbols, _texts) = parse_file(source, "ruby", "test.rb").unwrap();

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
        let (symbols, _texts) = parse_file(source, "ruby", "test.rb").unwrap();

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
        let (symbols, _texts) = parse_file(source, "ruby", "test.rb").unwrap();

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
        let (symbols, _texts) = parse_file(source, "ruby", "test.rb").unwrap();

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
        let (symbols, _texts) = parse_file(source, "ruby", "test.rb").unwrap();

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
        let (symbols, _texts) = parse_file(source, "ruby", "test.rb").unwrap();

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
        let (symbols, _texts) = parse_file(source, "ruby", "test.rb").unwrap();

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
        let (_symbols, texts) = parse_file(source, "ruby", "test.rb").unwrap();
        assert!(texts.iter().any(|t| t.kind == "comment"));
    }

    #[test]
    fn test_ruby_visibility_markers() {
        let source = b"def public_method
end

def _internal_method
end";
        let (symbols, _texts) = parse_file(source, "ruby", "test.rb").unwrap();

        let public = find_sym(&symbols, "public_method");
        assert_eq!(public.visibility.as_deref(), Some("public"));

        let internal = find_sym(&symbols, "_internal_method");
        assert_eq!(internal.visibility.as_deref(), Some("private"));
    }
}
