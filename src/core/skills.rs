//! Skills system for Enclave

use crate::core::approval::PermissionMode;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Skill definition representing a discoverable agent capability
#[derive(Debug, Clone)]
pub struct Skill {
    /// Human-readable skill name
    pub name: String,
    /// Brief description of what the skill does
    pub description: Option<String>,
    /// Slash command to invoke the skill (e.g., /review, /debug)
    pub command: String,
    /// Optional model override for this skill
    pub model: Option<String>,
    /// Optional permission mode override
    pub permission_mode: Option<PermissionMode>,
    /// Execution context mode
    pub context: SkillContextMode,
    /// Working directory for skill execution
    pub directory: PathBuf,
    /// Full skill instructions/body
    pub body: String,
    /// Source of the skill definition
    pub source: SkillSource,
}

/// Context mode for skill execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum SkillContextMode {
    /// Execute inline within current context
    Inline,
    /// Fork a new context for execution
    Fork,
}

impl Default for SkillContextMode {
    fn default() -> Self {
        SkillContextMode::Inline
    }
}

/// Source location of a skill definition
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    /// Discovered from AGENTS.md file
    AgentsMd,
    /// Discovered from filesystem (e.g., .nca/skills/, ~/.claude/skills/)
    FileSystem,
}

/// Frontmatter structure for filesystem skill files
#[derive(Debug, Default, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    command: Option<String>,
    model: Option<String>,
    permission_mode: Option<PermissionMode>,
    context: Option<SkillContextMode>,
}

/// Skill catalog for discovering and managing available skills
pub struct SkillCatalog;

impl SkillCatalog {
    /// Discover all skills from standard locations
    pub fn discover(
        workspace_root: &Path,
        skill_directories: &[PathBuf],
    ) -> Result<Vec<Skill>, String> {
        let mut roots = Vec::new();

        // Add user-level skill directories
        if let Some(home) = env::var_os("HOME") {
            let home = PathBuf::from(home);
            roots.push(home.join(".nca/skills"));
            roots.push(home.join(".claude/skills"));
        }

        // Add custom skill directories
        for dir in skill_directories {
            if dir.is_absolute() {
                roots.push(dir.clone());
            } else {
                roots.push(workspace_root.join(dir));
            }
        }

        let mut skills = Vec::new();

        // Parse AGENTS.md first (takes precedence on conflicts)
        if let Ok(agents_skills) = parse_agents_md(workspace_root) {
            skills.extend(agents_skills);
        }

        // Then add filesystem skills
        for root in &roots {
            if !root.exists() {
                continue;
            }
            let entries = std::fs::read_dir(root)
                .map_err(|err| format!("failed to read skills dir {}: {err}", root.display()))?;

            for entry in entries.flatten() {
                let path = entry.path();
                let skill_file = if path.is_dir() {
                    path.join("SKILL.md")
                } else if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
                    path.clone()
                } else {
                    continue;
                };

                if !skill_file.exists() {
                    continue;
                }

                if let Ok(skill) = parse_skill_file(&skill_file) {
                    if !skills
                        .iter()
                        .any(|existing: &Skill| existing.command == skill.command)
                    {
                        skills.push(skill);
                    }
                }
            }
        }

        skills.sort_by(|left, right| left.command.cmp(&right.command));
        Ok(skills)
    }
}

impl Skill {
    /// Generate a one-line summary for listing skills
    pub fn summary_line(&self) -> String {
        let source_tag = match self.source {
            SkillSource::AgentsMd => " [AGENTS.md]",
            SkillSource::FileSystem => "",
        };
        match &self.description {
            Some(description) => format!("/{:<14} {}{}", self.command, description, source_tag),
            None => format!("/{:<14} {}{}", self.command, self.name, source_tag),
        }
    }

    /// Generate a prompt fragment for invoking this skill with a task
    pub fn prompt_for_task(&self, task: &str) -> String {
        let mut prompt = format!(
            "Use the skill `{}`.\n\nSkill instructions:\n{}\n",
            self.command,
            self.expanded_body().trim()
        );
        if !task.trim().is_empty() {
            prompt.push_str(&format!("\nTask:\n{}\n", task.trim()));
        }
        prompt
    }

    /// Generate a detailed manifest entry for the skill
    pub fn manifest_summary(&self) -> String {
        let description = self
            .description
            .as_deref()
            .unwrap_or("No description provided.");
        let model = self.model.as_deref().unwrap_or("inherit");
        let permission_mode = self
            .permission_mode
            .map(|mode| format!("{mode:?}"))
            .unwrap_or_else(|| "inherit".into());
        let source_tag = match self.source {
            SkillSource::AgentsMd => " [AGENTS.md]",
            SkillSource::FileSystem => "",
        };
        format!(
            "- /{}: {}{}\n  model={model} permission_mode={permission_mode} context={:?}",
            self.command, description, source_tag, self.context
        )
    }

    /// Return a label for the skill source type
    pub fn source_label(&self) -> &'static str {
        match self.source {
            SkillSource::AgentsMd => "agents-md",
            SkillSource::FileSystem => "filesystem",
        }
    }

    /// Return the skill body with referenced supporting files inlined
    pub fn expanded_body(&self) -> String {
        let refs = extract_file_references(&self.body);
        if refs.is_empty() {
            return self.body.clone();
        }

        let mut expanded = self.body.clone();
        for ref_path in &refs {
            let clean = ref_path.trim_start_matches("./");

            if clean.contains("..") {
                continue;
            }

            let resolved = match resolve_skill_reference(ref_path, &self.directory) {
                Some(r) => r,
                None => continue,
            };

            if let Ok(content) = std::fs::read_to_string(&resolved) {
                expanded.push_str(&format!("\n\n===== {} =====\n\n{}", clean, content.trim()));
            }
        }

        expanded
    }
}

/// Parse a filesystem skill file with optional YAML frontmatter
fn parse_skill_file(path: &Path) -> Result<Skill, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;

    let directory = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let file_stem = directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("skill")
        .to_string();

    let (frontmatter, body) = split_frontmatter(&raw)?;

    let command = frontmatter.command.clone().unwrap_or_else(|| {
        slugify(
            &frontmatter
                .name
                .clone()
                .unwrap_or_else(|| file_stem.clone()),
        )
    });

    Ok(Skill {
        name: frontmatter.name.unwrap_or(file_stem),
        description: frontmatter.description,
        command,
        model: frontmatter.model,
        permission_mode: frontmatter.permission_mode,
        context: frontmatter.context.unwrap_or(SkillContextMode::Inline),
        directory,
        body: body.trim().to_string(),
        source: SkillSource::FileSystem,
    })
}

/// Split frontmatter from body content
fn split_frontmatter(raw: &str) -> Result<(SkillFrontmatter, String), String> {
    let rest = match raw.strip_prefix("---\n") {
        Some(r) => r,
        None => return Ok((SkillFrontmatter::default(), raw.to_string())),
    };
    let end = match rest.find("\n---\n") {
        Some(e) => e,
        None => return Ok((SkillFrontmatter::default(), raw.to_string())),
    };
    let yaml = &rest[..end];
    let body = &rest[end + 5..];
    let fm = serde_yaml::from_str::<SkillFrontmatter>(yaml)
        .map_err(|err| format!("failed to parse skill frontmatter: {err}"))?;
    Ok((fm, body.to_string()))
}

/// Convert a string to a valid command slug
fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Parse AGENTS.md as a skill manifest
fn parse_agents_md(workspace_root: &Path) -> Result<Vec<Skill>, String> {
    let agents_path = workspace_root.join("AGENTS.md");
    if !agents_path.exists() {
        return Ok(Vec::new());
    }

    let raw = std::fs::read_to_string(&agents_path)
        .map_err(|err| format!("failed to read AGENTS.md: {err}"))?;

    let mut skills = Vec::new();
    let mut current_heading = String::new();
    let mut current_content = String::new();
    let mut in_frontmatter = false;
    let mut frontmatter_lines = Vec::new();

    for line in raw.lines() {
        if line.starts_with("## ") {
            if !current_heading.is_empty() {
                if let Some(skill) = build_skill_from_section(
                    &current_heading,
                    &frontmatter_lines.join("\n"),
                    current_content.trim(),
                    workspace_root,
                ) {
                    skills.push(skill);
                }
                frontmatter_lines.clear();
                in_frontmatter = false;
            }
            current_heading = line.trim_start_matches("## ").trim().to_string();
            current_content.clear();
        } else if line.trim().is_empty() {
            if !current_content.is_empty() || !current_heading.is_empty() {
                current_content.push('\n');
            }
        } else if line.starts_with("- ") && !in_frontmatter && frontmatter_lines.is_empty() {
            let trimmed = line.trim_start_matches("- ");
            if trimmed.starts_with("model=")
                || trimmed.starts_with("permission_mode=")
                || trimmed.starts_with("context=")
            {
                in_frontmatter = true;
                frontmatter_lines.push(trimmed);
            } else {
                current_content.push_str(line);
                current_content.push('\n');
            }
        } else if in_frontmatter {
            if line.starts_with("- ") {
                frontmatter_lines.push(line.trim_start_matches("- "));
            } else {
                in_frontmatter = false;
                current_content.push_str(line);
                current_content.push('\n');
            }
        } else {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    if !current_heading.is_empty() {
        if let Some(skill) = build_skill_from_section(
            &current_heading,
            &frontmatter_lines.join("\n"),
            current_content.trim(),
            workspace_root,
        ) {
            skills.push(skill);
        }
    }

    Ok(skills)
}

/// Build a skill from a parsed AGENTS.md section
fn build_skill_from_section(
    heading: &str,
    frontmatter: &str,
    body: &str,
    workspace_root: &Path,
) -> Option<Skill> {
    let command = slugify(heading);
    if command.is_empty() {
        return None;
    }

    let mut model = None;
    let mut permission_mode = None;
    let mut context = SkillContextMode::Inline;

    for line in frontmatter.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        for directive in line.split_whitespace() {
            if directive.starts_with("model=") {
                let val = directive.trim_start_matches("model=").trim();
                if val != "inherit" {
                    model = Some(val.to_string());
                }
            } else if directive.starts_with("permission_mode=") {
                let val = directive.trim_start_matches("permission_mode=").trim();
                permission_mode = parse_permission_mode_str(val);
            } else if directive.starts_with("context=") {
                let val = directive
                    .trim_start_matches("context=")
                    .trim()
                    .to_lowercase();
                if val == "fork" {
                    context = SkillContextMode::Fork;
                }
            }
        }
    }

    Some(Skill {
        name: heading.to_string(),
        description: Some(section_description(heading, body)),
        command,
        model,
        permission_mode,
        context,
        directory: workspace_root.to_path_buf(),
        body: body.to_string(),
        source: SkillSource::AgentsMd,
    })
}

/// Extract description from section body
fn section_description(heading: &str, body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| {
            line.strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))
                .unwrap_or(line)
                .trim()
                .to_string()
        })
        .unwrap_or_else(|| heading.to_string())
}

/// Parse permission mode string
fn parse_permission_mode_str(raw: &str) -> Option<PermissionMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "plan" => Some(PermissionMode::Plan),
        "accept-edits" | "accept_edits" => Some(PermissionMode::AcceptEdits),
        "dont-ask" | "dont_ask" => Some(PermissionMode::DontAsk),
        "bypass-permissions" | "bypass_permissions" => Some(PermissionMode::BypassPermissions),
        _ => None,
    }
}

/// Resolve a file reference to an absolute path
fn resolve_skill_reference(reference: &str, skill_directory: &Path) -> Option<PathBuf> {
    let clean = reference.trim_start_matches("./");

    if clean.contains("..") {
        return None;
    }

    let level1 = skill_directory.join(clean);
    if level1.is_file() {
        return Some(level1);
    }

    if let Some(catalog_root) = skill_directory.parent() {
        let skill_dir_name = skill_directory
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let is_likely_skill_subdir =
            !skill_dir_name.is_empty() && skill_dir_name != "." && skill_directory != catalog_root;

        if is_likely_skill_subdir {
            let level2 = catalog_root.join(clean);
            if level2.is_file() {
                return Some(level2);
            }

            if let Some(stripped) = clean.strip_prefix("skills/") {
                if !stripped.contains("..") {
                    let level3 = catalog_root.join(stripped);
                    if level3.is_file() {
                        return Some(level3);
                    }
                }
            }
        }
    }

    None
}

// Regex patterns for extracting file references
static DOT_SLASH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\./([a-zA-Z0-9_\-./]+)").unwrap());

static AT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@([a-zA-Z0-9_\-./]+)").unwrap());

static BACKTICK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`([a-zA-Z0-9_\-./]+)`").unwrap());

static EXCLUDED_NAMES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "AGENTS.md",
        "SKILL.md",
        ".gitignore",
        "README.md",
        "LICENSE",
        "package.json",
        "Cargo.toml",
        "pyproject.toml",
    ])
});

fn extract_file_references(body: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut seen = HashSet::new();

    for cap in DOT_SLASH_RE.captures_iter(body) {
        if let Some(m) = cap.get(1) {
            let path = m.as_str().to_string();
            if !is_excluded(&path) && !path.contains("..") && seen.insert(path.clone()) {
                refs.push(format!("./{}", path));
            }
        }
    }

    for cap in AT_RE.captures_iter(body) {
        if let Some(m) = cap.get(1) {
            let path = m.as_str().to_string();
            if !is_excluded(&path) && !path.contains("..") && seen.insert(path.clone()) {
                refs.push(path);
            }
        }
    }

    for cap in BACKTICK_RE.captures_iter(body) {
        if let Some(m) = cap.get(1) {
            let path = m.as_str().to_string();
            if !is_excluded(&path) && !path.contains("..") && seen.insert(path.clone()) {
                refs.push(path);
            }
        }
    }

    refs
}

fn is_excluded(path: &str) -> bool {
    if let Some(name) = path.split('/').last() {
        EXCLUDED_NAMES.contains(name)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Code Review"), "code-review");
        assert_eq!(slugify("Debug Error"), "debug-error");
        assert_eq!(slugify("simple"), "simple");
        assert_eq!(slugify("Test  Multiple   Spaces"), "test-multiple-spaces");
    }

    #[test]
    fn test_split_frontmatter_with_frontmatter() {
        let raw = "name: review\ndescription: Review code\ncommand: review\n---\nReview body here";
        let (fm, body) = split_frontmatter(raw).unwrap();
        assert_eq!(fm.name, Some("review".to_string()));
        assert_eq!(fm.description, Some("Review code".to_string()));
        assert_eq!(fm.command, Some("review".to_string()));
        assert_eq!(body.trim(), "Review body here");
    }

    #[test]
    fn test_split_frontmatter_without_frontmatter() {
        let raw = "Just plain content\nwithout frontmatter.";
        let (fm, body) = split_frontmatter(raw).unwrap();
        assert_eq!(fm.name, None);
        assert_eq!(body, raw);
    }

    #[test]
    fn test_section_description() {
        assert_eq!(
            section_description("Code Review", "- Review code for bugs"),
            "Review code for bugs"
        );
        assert_eq!(
            section_description("Debug", "No bullets here"),
            "No bullets here"
        );
    }

    #[test]
    fn test_parse_permission_mode_str() {
        assert_eq!(
            parse_permission_mode_str("plan"),
            Some(PermissionMode::Plan)
        );
        assert_eq!(
            parse_permission_mode_str("bypass-permissions"),
            Some(PermissionMode::BypassPermissions)
        );
        assert_eq!(parse_permission_mode_str("unknown"), None);
    }

    #[test]
    fn test_excluded_names() {
        assert!(is_excluded("AGENTS.md"));
        assert!(is_excluded("README.md"));
        assert!(!is_excluded("src/main.rs"));
    }

    #[test]
    fn test_resolve_skill_reference_rejects_traversal() {
        let skill_dir = PathBuf::from("/skills/review");
        let resolved = resolve_skill_reference("../etc/passwd", &skill_dir);
        assert!(resolved.is_none());
    }
}
