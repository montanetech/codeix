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

        _ => anyhow::bail!("unsupported language: {name}"),
    }
}

/// Detect the language from a file extension.
pub fn detect_language(extension: &str) -> Option<&'static str> {
    match extension {
        "py" => Some("python"),
        "rs" => Some("rust"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("tsx"),
        _ => None,
    }
}
