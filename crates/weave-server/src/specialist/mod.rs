//! Specialist loading from markdown files with YAML frontmatter.
//!
//! Specialists are defined as `.md` files in a directory. Each file contains
//! YAML frontmatter (between `---` delimiters) with metadata, and a markdown
//! body that serves as the system prompt.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};

/// A specialist definition loaded from a markdown file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Specialist {
    /// Unique specialist identifier (used as HashMap key and session.specialist_id).
    pub name: String,
    /// Human-readable description of the specialist's purpose.
    pub description: String,
    /// Optional model override for this specialist.
    /// Consumed by `resolve_model` in session service (feat-011).
    #[serde(default)]
    pub model: Option<String>,
    /// Optional tool profile name.
    /// Will be consumed by ToolRegistry for tool filtering (feat-012).
    #[serde(default)]
    pub tool_profile: Option<String>,
    /// Optional tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Markdown body extracted after the frontmatter — used as system prompt.
    #[serde(skip)]
    pub system_prompt: String,
}

/// A registry of loaded specialists, keyed by name.
///
/// Specialists are loaded once at startup from markdown files and are
/// immutable after that. The registry provides lookup by name for
/// system prompt injection during session prompt handling.
pub struct SpecialistRegistry {
    specialists: HashMap<String, Specialist>,
}

impl SpecialistRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            specialists: HashMap::new(),
        }
    }

    /// Load specialists from `*.md` files in the given directory.
    ///
    /// Returns `(loaded_count, skipped_count)`. Files that fail to parse,
    /// have missing required fields, or contain invalid UTF-8 are skipped
    /// with a `tracing::warn!` log. If the directory doesn't exist, returns
    /// `(0, 0)` without error.
    pub fn load_from_dir(&mut self, dir: &Path) -> (usize, usize) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => {
                info!(
                    "Specialists directory not found, skipping: {}",
                    dir.display()
                );
                return (0, 0);
            }
        };

        let mut loaded = 0;
        let mut skipped = 0;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|ext| ext == "md") {
                continue;
            }

            match parse_specialist_file(&path) {
                Ok(specialist) => {
                    let name = specialist.name.clone();
                    if self.specialists.contains_key(&name) {
                        warn!(
                            specialist = %name,
                            path = %path.display(),
                            "Duplicate specialist name, overwriting"
                        );
                    }
                    self.specialists.insert(name, specialist);
                    loaded += 1;
                }
                Err(e) => {
                    warn!(
                        path = %path.display(),
                        error = %e,
                        "Skipping specialist file"
                    );
                    skipped += 1;
                }
            }
        }

        info!(loaded, skipped, "Specialists loaded");
        (loaded, skipped)
    }

    /// Look up a specialist by name.
    pub fn get_by_name(&self, name: &str) -> Option<&Specialist> {
        self.specialists.get(name)
    }

    /// Return the number of loaded specialists.
    pub fn count(&self) -> usize {
        self.specialists.len()
    }

    /// Insert a specialist into the registry. Used for testing and runtime registration.
    pub fn insert(&mut self, specialist: Specialist) {
        self.specialists.insert(specialist.name.clone(), specialist);
    }

    /// Return all loaded specialists.
    pub fn all(&self) -> Vec<&Specialist> {
        self.specialists.values().collect()
    }
}

impl Default for SpecialistRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a single specialist markdown file.
///
/// Expected format:
/// ```markdown
/// ---
/// name: my-specialist
/// description: Does something useful
/// model: claude-sonnet-4-20250514
/// tool_profile: implementation
/// tags: [coding, testing]
/// ---
///
/// System prompt content goes here.
/// ```
fn parse_specialist_file(path: &Path) -> Result<Specialist, ParseError> {
    let bytes = std::fs::read(path).map_err(|e| ParseError::Io(e.to_string()))?;
    let content = String::from_utf8(bytes).map_err(|_| ParseError::InvalidUtf8)?;
    parse_specialist_content(&content)
}

/// Parse specialist content from a string.
fn parse_specialist_content(content: &str) -> Result<Specialist, ParseError> {
    let trimmed = content.trim();

    // Find the opening ---
    let after_open = trimmed
        .strip_prefix("---")
        .ok_or(ParseError::MissingFrontmatter)?;

    // Find the closing --- (must start at a line boundary to avoid matching
    // `---` inside YAML values like `description: Handles code --- reviews`)
    let (yaml_str, body) = after_open
        .split_once("\n---")
        .ok_or(ParseError::MissingFrontmatter)?;

    // Parse YAML frontmatter using serde_yaml::Value to avoid a duplicate struct
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(yaml_str).map_err(|e| ParseError::YamlError(e.to_string()))?;

    // Extract and validate required fields
    let name = yaml
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or(ParseError::MissingField("name"))?
        .to_string();

    let description = yaml
        .get("description")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or(ParseError::MissingField("description"))?
        .to_string();

    // Extract optional fields
    let model = yaml
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let tool_profile = yaml
        .get("tool_profile")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let tags = yaml
        .get("tags")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let system_prompt = body.trim().to_string();

    Ok(Specialist {
        name,
        description,
        model,
        tool_profile,
        tags,
        system_prompt,
    })
}

/// Errors that can occur when parsing a specialist file.
#[derive(Debug)]
enum ParseError {
    /// File I/O error.
    Io(String),
    /// File is not valid UTF-8.
    InvalidUtf8,
    /// Missing `---` frontmatter delimiters.
    MissingFrontmatter,
    /// YAML parsing error.
    YamlError(String),
    /// A required field is missing or empty.
    MissingField(&'static str),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Io(e) => write!(f, "I/O error: {e}"),
            ParseError::InvalidUtf8 => write!(f, "invalid UTF-8"),
            ParseError::MissingFrontmatter => {
                write!(f, "missing --- frontmatter delimiters")
            }
            ParseError::YamlError(e) => write!(f, "YAML error: {e}"),
            ParseError::MissingField(field) => write!(f, "missing required field: {field}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_specialist_frontmatter_parse() {
        let content = r#"---
name: dev-crafter
description: A coding specialist
model: claude-sonnet-4-20250514
tool_profile: implementation
tags: [coding, rust]
---

You are a senior software engineer.
Write clean, well-tested code.
"#;

        let specialist = parse_specialist_content(content).unwrap();
        assert_eq!(specialist.name, "dev-crafter");
        assert_eq!(specialist.description, "A coding specialist");
        assert_eq!(
            specialist.model,
            Some("claude-sonnet-4-20250514".to_string())
        );
        assert_eq!(specialist.tool_profile, Some("implementation".to_string()));
        assert_eq!(specialist.tags, vec!["coding", "rust"]);
        assert!(specialist
            .system_prompt
            .contains("senior software engineer"));
        assert!(specialist.system_prompt.contains("well-tested code"));
    }

    #[test]
    fn test_specialist_frontmatter_minimal() {
        let content = r#"---
name: minimal
description: Minimal specialist
---

Just a prompt.
"#;

        let specialist = parse_specialist_content(content).unwrap();
        assert_eq!(specialist.name, "minimal");
        assert_eq!(specialist.description, "Minimal specialist");
        assert_eq!(specialist.model, None);
        assert_eq!(specialist.tool_profile, None);
        assert!(specialist.tags.is_empty());
        assert_eq!(specialist.system_prompt, "Just a prompt.");
    }

    #[test]
    fn test_specialist_frontmatter_with_dashes_in_yaml() {
        // Verifies that `---` inside YAML values doesn't break parsing
        let content = r#"---
name: reviewer
description: Handles code --- reviews
model: claude-sonnet-4-20250514
---

You review code thoroughly.
"#;

        let specialist = parse_specialist_content(content).unwrap();
        assert_eq!(specialist.name, "reviewer");
        assert_eq!(specialist.description, "Handles code --- reviews");
        assert_eq!(
            specialist.model,
            Some("claude-sonnet-4-20250514".to_string())
        );
        assert_eq!(specialist.system_prompt, "You review code thoroughly.");
    }

    #[test]
    fn test_specialist_malformed_yaml() {
        // Missing --- delimiters
        assert!(matches!(
            parse_specialist_content("no frontmatter here"),
            Err(ParseError::MissingFrontmatter)
        ));

        // Only one --- delimiter
        assert!(matches!(
            parse_specialist_content("---\nname: test\n"),
            Err(ParseError::MissingFrontmatter)
        ));

        // Invalid YAML
        assert!(matches!(
            parse_specialist_content("---\n{: invalid yaml\n---\nbody"),
            Err(ParseError::YamlError(_))
        ));

        // Missing name field
        assert!(matches!(
            parse_specialist_content("---\ndescription: test\n---\nbody"),
            Err(ParseError::MissingField("name"))
        ));

        // Missing description field
        assert!(matches!(
            parse_specialist_content("---\nname: test\n---\nbody"),
            Err(ParseError::MissingField("description"))
        ));

        // Empty name field
        assert!(matches!(
            parse_specialist_content("---\nname: \"\"\ndescription: test\n---\nbody"),
            Err(ParseError::MissingField("name"))
        ));

        // Empty description field
        assert!(matches!(
            parse_specialist_content("---\nname: test\ndescription: \"\"\n---\nbody"),
            Err(ParseError::MissingField("description"))
        ));
    }

    #[test]
    fn test_specialist_loading() {
        let dir = std::env::temp_dir().join("weave-specialist-test-loading");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Write valid specialist files
        std::fs::write(
            dir.join("coder.md"),
            r#"---
name: coder
description: Writes code
tags: [coding]
---
Write good code."#,
        )
        .unwrap();

        std::fs::write(
            dir.join("reviewer.md"),
            r#"---
name: reviewer
description: Reviews code
model: claude-opus-4-20250514
---
Review thoroughly."#,
        )
        .unwrap();

        // Write a non-.md file (should be ignored)
        std::fs::write(dir.join("notes.txt"), "not a specialist").unwrap();

        // Write a malformed file (should be skipped)
        std::fs::write(dir.join("broken.md"), "no frontmatter here").unwrap();

        let mut registry = SpecialistRegistry::new();
        let (loaded, skipped) = registry.load_from_dir(&dir);

        assert_eq!(loaded, 2);
        assert_eq!(skipped, 1);
        assert_eq!(registry.count(), 2);

        let coder = registry.get_by_name("coder").unwrap();
        assert_eq!(coder.description, "Writes code");
        assert_eq!(coder.tags, vec!["coding"]);
        assert_eq!(coder.system_prompt, "Write good code.");

        let reviewer = registry.get_by_name("reviewer").unwrap();
        assert_eq!(reviewer.description, "Reviews code");
        assert_eq!(reviewer.model, Some("claude-opus-4-20250514".to_string()));
        assert_eq!(reviewer.system_prompt, "Review thoroughly.");

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_specialist_loading_missing_dir() {
        let mut registry = SpecialistRegistry::new();
        let (loaded, skipped) =
            registry.load_from_dir(Path::new("/nonexistent/path/to/specialists"));
        assert_eq!(loaded, 0);
        assert_eq!(skipped, 0);
        assert_eq!(registry.count(), 0);
    }
}
