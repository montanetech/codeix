use anyhow::Result;
use tree_sitter::Language;

/// Return the tree-sitter [`Language`] for the given language name.
///
/// Languages are feature-gated — only grammars enabled at compile time are
/// available.
pub fn get_language(name: &str) -> Result<Language> {
    match name {
        #[cfg(feature = "lang-python")]
        "python" => Ok(tree_sitter_python::LANGUAGE.into()),

        #[cfg(feature = "lang-rust")]
        "rust" => Ok(tree_sitter_rust::LANGUAGE.into()),

        #[cfg(feature = "lang-javascript")]
        "javascript" => Ok(tree_sitter_javascript::LANGUAGE.into()),

        #[cfg(feature = "lang-typescript")]
        "typescript" => Ok(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),

        #[cfg(feature = "lang-typescript")]
        "tsx" => Ok(tree_sitter_typescript::LANGUAGE_TSX.into()),

        #[cfg(feature = "lang-go")]
        "go" => Ok(tree_sitter_go::LANGUAGE.into()),

        #[cfg(feature = "lang-java")]
        "java" => Ok(tree_sitter_java::LANGUAGE.into()),

        #[cfg(feature = "lang-c")]
        "c" => Ok(tree_sitter_c::LANGUAGE.into()),

        #[cfg(feature = "lang-cpp")]
        "cpp" => Ok(tree_sitter_cpp::LANGUAGE.into()),

        #[cfg(feature = "lang-ruby")]
        "ruby" => Ok(tree_sitter_ruby::LANGUAGE.into()),

        #[cfg(feature = "lang-csharp")]
        "csharp" => Ok(tree_sitter_c_sharp::LANGUAGE.into()),

        // Markdown uses custom parser (tree-sitter-md) with MarkdownParser,
        // but we still register the block language for consistency
        #[cfg(feature = "lang-markdown")]
        "markdown" => Ok(tree_sitter_md::LANGUAGE.into()),

        _ => anyhow::bail!("unsupported language: {name}"),
    }
}

/// Detect the language from a file extension.
pub fn detect_language(extension: &str) -> Option<&'static str> {
    match extension {
        "py" | "pyi" | "pyw" => Some("python"),
        "rs" => Some("rust"),
        "js" | "mjs" | "cjs" | "jsx" => Some("javascript"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "go" => Some("go"),
        "java" => Some("java"),
        "c" | "h" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" | "h++" => Some("cpp"),
        "rb" | "rake" | "gemspec" => Some("ruby"),
        "cs" => Some("csharp"),
        "html" | "htm" => Some("html"),
        "vue" => Some("vue"),
        "svelte" => Some("svelte"),
        "astro" => Some("astro"),
        "md" | "markdown" => Some("markdown"),
        _ => None,
    }
}
