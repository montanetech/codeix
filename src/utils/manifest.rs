//! Package manifest parsing for project metadata extraction.
//!
//! Parses common package manifests (package.json, Cargo.toml, pyproject.toml, go.mod, pom.xml, *.gemspec)
//! and returns both fixed metadata (name, description) and list of manifest files found.

use std::fs;
use std::path::Path;

use serde::Serialize;

/// Fixed project metadata extracted from manifests.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectMetadata {
    /// Human-readable project name (from first manifest found, or directory name)
    pub name: String,
    /// Project description (from first manifest with description)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// List of manifest files found (e.g., ["package.json", "Cargo.toml"])
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manifest_files: Vec<String>,
}

/// Extract project metadata from all manifests found in the project root.
///
/// Checks for: package.json, Cargo.toml, pyproject.toml, go.mod, pom.xml, *.gemspec
/// Returns fixed metadata (name, description) plus list of manifest files found.
pub fn extract_metadata(project_root: &Path) -> ProjectMetadata {
    let mut manifest_files = Vec::new();
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;

    // Try each manifest type
    if let Some((n, d)) = try_package_json(project_root) {
        if name.is_none() {
            name = Some(n);
        }
        if description.is_none() {
            description = d;
        }
        manifest_files.push("package.json".to_string());
    }

    if let Some((n, d)) = try_cargo_toml(project_root) {
        if name.is_none() {
            name = Some(n);
        }
        if description.is_none() {
            description = d;
        }
        manifest_files.push("Cargo.toml".to_string());
    }

    if let Some((n, d)) = try_pyproject_toml(project_root) {
        if name.is_none() {
            name = Some(n);
        }
        if description.is_none() {
            description = d;
        }
        manifest_files.push("pyproject.toml".to_string());
    }

    if let Some(n) = try_go_mod(project_root) {
        if name.is_none() {
            name = Some(n);
        }
        // go.mod has no description
        manifest_files.push("go.mod".to_string());
    }

    if let Some((n, d)) = try_pom_xml(project_root) {
        if name.is_none() {
            name = Some(n);
        }
        if description.is_none() {
            description = d;
        }
        manifest_files.push("pom.xml".to_string());
    }

    if let Some((gemspec_file, n, d)) = try_gemspec(project_root) {
        if name.is_none() {
            name = Some(n);
        }
        if description.is_none() {
            description = d;
        }
        manifest_files.push(gemspec_file);
    }

    // Fallback: directory name
    let name = name.unwrap_or_else(|| {
        project_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string()
    });

    ProjectMetadata {
        name,
        description,
        manifest_files,
    }
}

/// Parse package.json and return (name, description)
/// Handles both regular packages and monorepo roots (private: true without name).
fn try_package_json(root: &Path) -> Option<(String, Option<String>)> {
    let path = root.join("package.json");
    let content = fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    // Try to get name from the package
    if let Some(name) = json.get("name").and_then(|n| n.as_str()) {
        let description = json
            .get("description")
            .and_then(|d| d.as_str())
            .map(String::from);
        return Some((name.to_string(), description));
    }

    // Handle monorepo roots (private: true without name) - use directory name
    if json.get("private") == Some(&serde_json::Value::Bool(true)) {
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("package")
            .to_string();
        let description = json
            .get("description")
            .and_then(|d| d.as_str())
            .map(String::from);
        return Some((name, description));
    }

    None
}

/// Parse Cargo.toml and return (name, description)
/// Handles both package manifests ([package]) and workspace manifests ([workspace]).
fn try_cargo_toml(root: &Path) -> Option<(String, Option<String>)> {
    let path = root.join("Cargo.toml");
    let content = fs::read_to_string(path).ok()?;
    let toml_value: toml::Value = toml::from_str(&content).ok()?;

    // Try [package] first (standard crate)
    if let Some(package) = toml_value.get("package")
        && let Some(name) = package.get("name").and_then(|n| n.as_str())
    {
        let description = package
            .get("description")
            .and_then(|d| d.as_str())
            .map(String::from);
        return Some((name.to_string(), description));
    }

    // Try [workspace] (workspace root) - use directory name, no description
    if toml_value.get("workspace").is_some() {
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_string();
        return Some((name, None));
    }

    None
}

/// Parse pyproject.toml and return (name, description)
fn try_pyproject_toml(root: &Path) -> Option<(String, Option<String>)> {
    let path = root.join("pyproject.toml");
    let content = fs::read_to_string(path).ok()?;
    let toml_value: toml::Value = toml::from_str(&content).ok()?;

    // Try project.name (PEP 621)
    if let Some(project) = toml_value.get("project")
        && let Some(name) = project.get("name").and_then(|n| n.as_str())
    {
        let description = project
            .get("description")
            .and_then(|d| d.as_str())
            .map(String::from);
        return Some((name.to_string(), description));
    }

    // Try tool.poetry.name (Poetry)
    if let Some(tool) = toml_value.get("tool")
        && let Some(poetry) = tool.get("poetry")
        && let Some(name) = poetry.get("name").and_then(|n| n.as_str())
    {
        let description = poetry
            .get("description")
            .and_then(|d| d.as_str())
            .map(String::from);
        return Some((name.to_string(), description));
    }

    None
}

/// Parse go.mod and return the module name
/// go.mod has no description field
fn try_go_mod(root: &Path) -> Option<String> {
    let path = root.join("go.mod");
    let content = fs::read_to_string(path).ok()?;

    let mut module_path: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("module ") {
            module_path = line.strip_prefix("module ").map(|s| s.trim().to_string());
            break;
        }
    }

    let module_path = module_path?;
    // Use last segment as name, filter out empty strings
    module_path
        .split('/')
        .next_back()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Parse pom.xml (Maven) and return (name, description)
/// Uses simple regex-based extraction to avoid adding an XML dependency.
fn try_pom_xml(root: &Path) -> Option<(String, Option<String>)> {
    let path = root.join("pom.xml");
    let content = fs::read_to_string(path).ok()?;

    // Extract artifactId as name (prefer <name> if present at top level)
    // We look for top-level elements, not nested in <parent> or <dependency>
    let name = extract_xml_element(&content, "name")
        .or_else(|| extract_xml_element(&content, "artifactId"))?;

    let description = extract_xml_element(&content, "description");

    Some((name, description))
}

/// Simple XML element extraction (first occurrence, top-level only).
/// This is a basic implementation that works for common pom.xml structures.
fn extract_xml_element(content: &str, tag: &str) -> Option<String> {
    let open_tag = format!("<{}>", tag);
    let close_tag = format!("</{}>", tag);

    let start = content.find(&open_tag)? + open_tag.len();
    let end = content[start..].find(&close_tag)? + start;

    let value = content[start..end].trim();
    if value.is_empty() || value.starts_with('<') {
        // Empty or contains nested elements
        None
    } else {
        Some(value.to_string())
    }
}

/// Parse *.gemspec (Ruby gem) and return (filename, name, description)
/// Looks for .name and .summary/.description assignments.
fn try_gemspec(root: &Path) -> Option<(String, String, Option<String>)> {
    // Find *.gemspec file in the directory
    let gemspec_file = fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().is_some_and(|ext| ext == "gemspec"))?;

    let filename = gemspec_file.file_name().to_string_lossy().to_string();
    let content = fs::read_to_string(gemspec_file.path()).ok()?;

    // Extract name: look for .name = "..." or .name = '...'
    let name = extract_ruby_string_assignment(&content, "name")?;

    // Extract description: prefer .summary, fall back to .description
    let description = extract_ruby_string_assignment(&content, "summary")
        .or_else(|| extract_ruby_string_assignment(&content, "description"));

    Some((filename, name, description))
}

/// Extract a Ruby string assignment like `s.name = "value"` or `spec.name = 'value'`
fn extract_ruby_string_assignment(content: &str, field: &str) -> Option<String> {
    // Pattern: <var>.field = "value" or <var>.field = 'value'
    let field_pattern = format!(".{}", field);

    for line in content.lines() {
        let line = line.trim();
        // Find .field in the line (e.g., "s.name" or "spec.name")
        if let Some(pos) = line.find(&field_pattern) {
            let rest = &line[pos + field_pattern.len()..];
            let rest = rest.trim();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim();
                // Handle quoted strings
                if let Some(value) = extract_quoted_string(rest) {
                    return Some(value);
                }
            }
        }
    }
    None
}

/// Extract a quoted string (single or double quotes)
fn extract_quoted_string(s: &str) -> Option<String> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    } else if let Some(rest) = s.strip_prefix('\'') {
        let end = rest.find('\'')?;
        Some(rest[..end].to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_package_json_parsing() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"name": "my-app", "description": "A cool app", "version": "1.0.0"}"#,
        )
        .unwrap();

        let meta = extract_metadata(tmp.path());
        assert_eq!(meta.name, "my-app");
        assert_eq!(meta.description, Some("A cool app".into()));
        assert!(meta.manifest_files.contains(&"package.json".to_string()));
    }

    #[test]
    fn test_cargo_toml_parsing() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"
[package]
name = "my-crate"
version = "0.1.0"
description = "A Rust library"
"#,
        )
        .unwrap();

        let meta = extract_metadata(tmp.path());
        assert_eq!(meta.name, "my-crate");
        assert_eq!(meta.description, Some("A Rust library".into()));
        assert!(meta.manifest_files.contains(&"Cargo.toml".to_string()));
    }

    #[test]
    fn test_pyproject_toml_pep621() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("pyproject.toml"),
            r#"
[project]
name = "my-python-pkg"
description = "A Python package"
version = "1.0.0"
"#,
        )
        .unwrap();

        let meta = extract_metadata(tmp.path());
        assert_eq!(meta.name, "my-python-pkg");
        assert_eq!(meta.description, Some("A Python package".into()));
        assert!(meta.manifest_files.contains(&"pyproject.toml".to_string()));
    }

    #[test]
    fn test_pyproject_toml_poetry() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("pyproject.toml"),
            r#"
[tool.poetry]
name = "poetry-pkg"
description = "A Poetry package"
version = "2.0.0"
"#,
        )
        .unwrap();

        let meta = extract_metadata(tmp.path());
        assert_eq!(meta.name, "poetry-pkg");
        assert_eq!(meta.description, Some("A Poetry package".into()));
    }

    #[test]
    fn test_go_mod_parsing() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("go.mod"),
            "module github.com/user/myrepo\n\ngo 1.21\n",
        )
        .unwrap();

        let meta = extract_metadata(tmp.path());
        assert_eq!(meta.name, "myrepo");
        assert_eq!(meta.description, None); // go.mod has no description
        assert!(meta.manifest_files.contains(&"go.mod".to_string()));
    }

    #[test]
    fn test_fallback_to_directory_name() {
        let tmp = TempDir::new().unwrap();
        // No manifest files

        let meta = extract_metadata(tmp.path());
        // Name should be the temp directory name (starts with '.')
        assert!(!meta.name.is_empty());
        assert_eq!(meta.description, None);
        assert!(meta.manifest_files.is_empty());
    }

    #[test]
    fn test_multiple_manifests() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"name": "npm-name", "description": "NPM desc"}"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"
[package]
name = "cargo-name"
description = "Cargo desc"
"#,
        )
        .unwrap();

        let meta = extract_metadata(tmp.path());
        // First found (npm) wins for name/description
        assert_eq!(meta.name, "npm-name");
        assert_eq!(meta.description, Some("NPM desc".into()));
        // But both manifest files are listed
        assert!(meta.manifest_files.contains(&"package.json".to_string()));
        assert!(meta.manifest_files.contains(&"Cargo.toml".to_string()));
    }

    #[test]
    fn test_first_description_wins() {
        let tmp = TempDir::new().unwrap();
        // package.json with name but no description
        fs::write(tmp.path().join("package.json"), r#"{"name": "npm-name"}"#).unwrap();
        // Cargo.toml with description
        fs::write(
            tmp.path().join("Cargo.toml"),
            r#"
[package]
name = "cargo-name"
description = "Cargo has the description"
"#,
        )
        .unwrap();

        let meta = extract_metadata(tmp.path());
        assert_eq!(meta.name, "npm-name"); // npm first
        assert_eq!(meta.description, Some("Cargo has the description".into())); // cargo provides description
    }

    #[test]
    fn test_pom_xml_parsing() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("pom.xml"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<project>
    <modelVersion>4.0.0</modelVersion>
    <groupId>com.example</groupId>
    <artifactId>my-java-app</artifactId>
    <version>1.0.0</version>
    <name>My Java Application</name>
    <description>A sample Java application</description>
</project>
"#,
        )
        .unwrap();

        let meta = extract_metadata(tmp.path());
        assert_eq!(meta.name, "My Java Application");
        assert_eq!(meta.description, Some("A sample Java application".into()));
        assert!(meta.manifest_files.contains(&"pom.xml".to_string()));
    }

    #[test]
    fn test_pom_xml_without_name() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("pom.xml"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<project>
    <groupId>com.example</groupId>
    <artifactId>simple-app</artifactId>
    <version>1.0.0</version>
</project>
"#,
        )
        .unwrap();

        let meta = extract_metadata(tmp.path());
        // Falls back to artifactId when <name> is not present
        assert_eq!(meta.name, "simple-app");
        assert_eq!(meta.description, None);
        assert!(meta.manifest_files.contains(&"pom.xml".to_string()));
    }

    #[test]
    fn test_gemspec_parsing() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("my_gem.gemspec"),
            r#"
Gem::Specification.new do |s|
  s.name        = "my_gem"
  s.version     = "1.0.0"
  s.summary     = "A sample Ruby gem"
  s.description = "A longer description of my gem"
  s.authors     = ["Test Author"]
end
"#,
        )
        .unwrap();

        let meta = extract_metadata(tmp.path());
        assert_eq!(meta.name, "my_gem");
        assert_eq!(meta.description, Some("A sample Ruby gem".into())); // uses summary
        assert!(meta.manifest_files.contains(&"my_gem.gemspec".to_string()));
    }

    #[test]
    fn test_gemspec_single_quotes() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("another.gemspec"),
            r#"
Gem::Specification.new do |spec|
  spec.name    = 'another-gem'
  spec.version = '2.0.0'
  spec.summary = 'Single quoted summary'
end
"#,
        )
        .unwrap();

        let meta = extract_metadata(tmp.path());
        assert_eq!(meta.name, "another-gem");
        assert_eq!(meta.description, Some("Single quoted summary".into()));
        assert!(meta.manifest_files.contains(&"another.gemspec".to_string()));
    }

    #[test]
    fn test_cargo_workspace_toml() {
        let tmp = TempDir::new().unwrap();
        // Create a subdirectory with a specific name
        let workspace_dir = tmp.path().join("my-workspace");
        fs::create_dir(&workspace_dir).unwrap();
        fs::write(
            workspace_dir.join("Cargo.toml"),
            r#"
[workspace]
resolver = "2"
members = ["crate-a", "crate-b"]
"#,
        )
        .unwrap();

        let meta = extract_metadata(&workspace_dir);
        assert_eq!(meta.name, "my-workspace"); // Uses directory name
        assert_eq!(meta.description, None); // Workspaces don't have description
        assert!(meta.manifest_files.contains(&"Cargo.toml".to_string()));
    }

    #[test]
    fn test_package_json_monorepo() {
        let tmp = TempDir::new().unwrap();
        // Create a subdirectory with a specific name
        let monorepo_dir = tmp.path().join("my-monorepo");
        fs::create_dir(&monorepo_dir).unwrap();
        fs::write(
            monorepo_dir.join("package.json"),
            r#"{"private": true, "workspaces": ["packages/*"]}"#,
        )
        .unwrap();

        let meta = extract_metadata(&monorepo_dir);
        assert_eq!(meta.name, "my-monorepo"); // Uses directory name
        assert_eq!(meta.description, None);
        assert!(meta.manifest_files.contains(&"package.json".to_string()));
    }
}
