use serde::{Deserialize, Serialize};

/// `index.json` manifest — top-level metadata for a `.codeindex/` directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexManifest {
    pub version: String,
    pub name: String,
    pub root: String,
    pub languages: Vec<String>,
}

/// One line in `files.jsonl` — metadata for a single indexed file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub lang: Option<String>,
    pub hash: String,
    pub lines: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
    /// File title extracted from the source (e.g., first heading, module doc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// File description extracted from the source (e.g., docstring, frontmatter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One line in `symbols.jsonl` — a symbol extracted from the AST.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolEntry {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub line: [u32; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
}

/// One line in `texts.jsonl` — a text block (docstring, comment, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEntry {
    pub file: String,
    pub kind: String,
    pub line: [u32; 2],
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
}
