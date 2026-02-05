//! Single File Component (SFC) preprocessor.
//!
//! Extracts `<script>` blocks from Vue, Svelte, and Astro files so they can
//! be fed to the existing TypeScript/JavaScript tree-sitter extractors.
//!
//! No external dependencies — uses simple byte-level scanning.

/// A script block extracted from an SFC file.
pub struct ScriptBlock {
    /// The script content (without the surrounding tags).
    pub content: Vec<u8>,
    /// The language to parse with: `"typescript"` or `"javascript"`.
    pub lang: &'static str,
    /// 1-based line number where the content starts in the original file.
    pub start_line: u32,
}

/// Extract script blocks from an SFC file based on its extension.
///
/// Returns an empty vec if no script blocks are found (e.g. template-only
/// components).
pub fn extract_script_blocks(source: &[u8], extension: &str) -> Vec<ScriptBlock> {
    match extension {
        "html" => extract_html_script_tags(source, "javascript"),
        "vue" => extract_vue_scripts(source),
        "svelte" => extract_svelte_scripts(source),
        "astro" => extract_astro_scripts(source),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Vue: <script> and <script setup>, with optional lang="ts"
// ---------------------------------------------------------------------------

fn extract_vue_scripts(source: &[u8]) -> Vec<ScriptBlock> {
    extract_html_script_tags(source, "javascript")
}

// ---------------------------------------------------------------------------
// Svelte: <script> with optional lang="ts"
// ---------------------------------------------------------------------------

fn extract_svelte_scripts(source: &[u8]) -> Vec<ScriptBlock> {
    extract_html_script_tags(source, "javascript")
}

// ---------------------------------------------------------------------------
// Astro: --- frontmatter --- (always TS) + optional <script> blocks
// ---------------------------------------------------------------------------

fn extract_astro_scripts(source: &[u8]) -> Vec<ScriptBlock> {
    let mut blocks = Vec::new();
    let text = String::from_utf8_lossy(source);

    // Extract frontmatter: content between first `---` and second `---`
    if let Some(first) = text.find("---") {
        let after_first = first + 3;
        if let Some(second) = text[after_first..].find("---") {
            // Skip leading newline after opening ---
            let mut fm_start = after_first;
            if fm_start < text.len() && text.as_bytes()[fm_start] == b'\n' {
                fm_start += 1;
            } else if fm_start + 1 < text.len()
                && text.as_bytes()[fm_start] == b'\r'
                && text.as_bytes()[fm_start + 1] == b'\n'
            {
                fm_start += 2;
            }
            let fm_end = after_first + second;
            let frontmatter = &text[fm_start..fm_end];

            // Count lines before the frontmatter content starts
            let start_line = count_newlines_in(&text[..fm_start]) + 1;

            if !frontmatter.trim().is_empty() {
                blocks.push(ScriptBlock {
                    content: frontmatter.as_bytes().to_vec(),
                    lang: "typescript",
                    start_line: start_line as u32,
                });
            }
        }
    }

    // Also extract any <script> tags in the body
    blocks.extend(extract_html_script_tags(source, "typescript"));

    blocks
}

// ---------------------------------------------------------------------------
// Shared: extract <script> ... </script> blocks from HTML-like content
// ---------------------------------------------------------------------------

/// Extract all `<script ...>...</script>` blocks from HTML-like source.
///
/// `default_lang` is used when no `lang="..."` attribute is present.
fn extract_html_script_tags(source: &[u8], default_lang: &str) -> Vec<ScriptBlock> {
    let mut blocks = Vec::new();
    let text = String::from_utf8_lossy(source);
    let text_lower = text.to_ascii_lowercase();

    let mut search_from = 0;

    while let Some(pos) = text_lower[search_from..].find("<script") {
        let tag_start = search_from + pos;

        // Make sure it's actually a tag (next char after "script" should be whitespace or >)
        let after_script = tag_start + 7; // len("<script")
        if after_script >= text.len() {
            break;
        }
        let next_char = text.as_bytes()[after_script];
        if next_char != b' '
            && next_char != b'\t'
            && next_char != b'\n'
            && next_char != b'\r'
            && next_char != b'>'
        {
            search_from = after_script;
            continue;
        }

        // Find the closing > of the opening tag
        let tag_close = match text[after_script..].find('>') {
            Some(pos) => after_script + pos,
            None => break,
        };

        // Extract the opening tag attributes
        let open_tag = &text[tag_start..=tag_close];

        // Detect language from lang="..." attribute
        let lang = detect_script_lang(open_tag, default_lang);

        // Content starts after the > (skip leading newline if present)
        let mut content_start = tag_close + 1;
        if content_start < text.len() && text.as_bytes()[content_start] == b'\n' {
            content_start += 1;
        } else if content_start + 1 < text.len()
            && text.as_bytes()[content_start] == b'\r'
            && text.as_bytes()[content_start + 1] == b'\n'
        {
            content_start += 2;
        }

        // Find the matching </script> — content ends where the close tag begins
        let content_end = match text_lower[content_start..].find("</script") {
            Some(pos) => content_start + pos,
            None => break,
        };

        let content = &text[content_start..content_end];

        // Calculate the 1-based start line of the content
        let start_line = count_newlines_in(&text[..content_start]) + 1;

        if !content.trim().is_empty() {
            blocks.push(ScriptBlock {
                content: content.as_bytes().to_vec(),
                lang,
                start_line: start_line as u32,
            });
        }

        // Move past the closing tag
        search_from = content_end;
        // Skip past </script>
        if let Some(pos) = text_lower[search_from..].find('>') {
            search_from += pos + 1;
        } else {
            break;
        }
    }

    blocks
}

/// Detect the script language from an opening `<script ...>` tag.
///
/// Looks for `lang="ts"`, `lang="typescript"`, `lang='ts'`, etc.
/// Returns `"typescript"` or `"javascript"`.
fn detect_script_lang(open_tag: &str, default_lang: &str) -> &'static str {
    let lower = open_tag.to_ascii_lowercase();

    // Match lang="ts" or lang="typescript" (with either quote style)
    if let Some(pos) = lower.find("lang=") {
        let after_eq = pos + 5;
        let rest = &lower[after_eq..];
        let rest = rest.trim_start_matches(['"', '\'']);

        if rest.starts_with("ts") || rest.starts_with("typescript") {
            return "typescript";
        }
        if rest.starts_with("js") || rest.starts_with("javascript") {
            return "javascript";
        }
    }

    // No lang attribute — use the default
    if default_lang == "typescript" {
        "typescript"
    } else {
        "javascript"
    }
}

/// Count the number of newline characters in a string slice.
fn count_newlines_in(s: &str) -> usize {
    s.bytes().filter(|&b| b == b'\n').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vue_basic() {
        let source = b"<template>\n  <div>hello</div>\n</template>\n\n<script setup lang=\"ts\">\nimport { ref } from 'vue'\nconst msg = ref('hi')\n</script>\n";
        let blocks = extract_script_blocks(source, "vue");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lang, "typescript");
        assert_eq!(blocks[0].start_line, 6);
        let content = String::from_utf8_lossy(&blocks[0].content);
        assert!(content.contains("import { ref }"));
        assert!(content.contains("const msg"));
    }

    #[test]
    fn test_vue_two_scripts() {
        let source = b"<script>\nexport default { name: 'Foo' }\n</script>\n\n<script setup lang=\"ts\">\nconst x = 1\n</script>\n";
        let blocks = extract_script_blocks(source, "vue");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].lang, "javascript");
        assert_eq!(blocks[0].start_line, 2);
        assert_eq!(blocks[1].lang, "typescript");
        assert_eq!(blocks[1].start_line, 6);
    }

    #[test]
    fn test_vue_no_script() {
        let source = b"<template>\n  <div>hello</div>\n</template>\n";
        let blocks = extract_script_blocks(source, "vue");
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_astro_frontmatter() {
        let source = b"---\nimport Layout from './Layout.astro'\nconst title = 'Hello'\n---\n\n<Layout title={title}>\n  <h1>Hello</h1>\n</Layout>\n";
        let blocks = extract_script_blocks(source, "astro");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lang, "typescript");
        assert_eq!(blocks[0].start_line, 2);
        let content = String::from_utf8_lossy(&blocks[0].content);
        assert!(content.contains("import Layout"));
    }

    #[test]
    fn test_svelte_basic() {
        let source = b"<script lang=\"ts\">\n  let count = 0\n  function inc() { count++ }\n</script>\n\n<button on:click={inc}>{count}</button>\n";
        let blocks = extract_script_blocks(source, "svelte");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lang, "typescript");
        assert_eq!(blocks[0].start_line, 2);
    }

    #[test]
    fn test_html_script_extraction() {
        let source = b"<!DOCTYPE html>\n<html>\n<head>\n<script>\nfunction greet(name) {\n  return 'Hello ' + name;\n}\n</script>\n</head>\n<body></body>\n</html>\n";
        let blocks = extract_script_blocks(source, "html");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lang, "javascript");
        assert_eq!(blocks[0].start_line, 5);
        let content = String::from_utf8_lossy(&blocks[0].content);
        assert!(content.contains("function greet"));
    }

    #[test]
    fn test_html_no_script() {
        let source = b"<!DOCTYPE html>\n<html><body><p>Hello</p></body></html>\n";
        let blocks = extract_script_blocks(source, "html");
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_html_typescript_script() {
        let source = b"<html>\n<body>\n<script lang=\"ts\">\nconst x: number = 42;\n</script>\n</body>\n</html>\n";
        let blocks = extract_script_blocks(source, "html");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lang, "typescript");
    }

    #[test]
    fn test_detect_lang_ts() {
        assert_eq!(
            detect_script_lang("<script lang=\"ts\">", "javascript"),
            "typescript"
        );
        assert_eq!(
            detect_script_lang("<script lang='typescript'>", "javascript"),
            "typescript"
        );
        assert_eq!(
            detect_script_lang("<script setup lang=\"ts\">", "javascript"),
            "typescript"
        );
    }

    #[test]
    fn test_detect_lang_default() {
        assert_eq!(detect_script_lang("<script>", "javascript"), "javascript");
        assert_eq!(
            detect_script_lang("<script setup>", "javascript"),
            "javascript"
        );
        assert_eq!(detect_script_lang("<script>", "typescript"), "typescript");
    }
}
