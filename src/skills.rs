//! The skills loader — a CORE bootstrap lens (design_doc §4.1, §11.13).
//!
//! It reads a static `skills/` directory and populates the twin's skill registry on
//! startup.  It is deliberately part of the trusted kernel, not a loadable skill: the
//! mechanism that installs skills can't itself be installed as one (chicken-and-egg),
//! so it lives in the core alongside the governed host capabilities.  Down the road
//! skills live in the graph and this becomes the migration path from the codebase.

use std::fs;
use std::path::Path;

pub struct Skill {
    pub name: String,
    pub description: String,
    pub dir: String,
    pub files: Vec<String>,
}

/// Discover every `<root>/<name>/SKILL.md` and read its manifest.
pub fn discover(root: &str) -> Vec<Skill> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let manifest = dir.join("SKILL.md");
        if !manifest.exists() {
            continue;
        }
        let text = fs::read_to_string(&manifest).unwrap_or_default();
        let (name, description) = parse_manifest(&text, &dir);
        let mut files: Vec<String> = fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|f| f.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        out.push(Skill {
            name,
            description,
            dir: dir.to_string_lossy().into_owned(),
            files,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// name + description from YAML frontmatter if present, else derive from the dir name
/// and the first meaningful line.
fn parse_manifest(text: &str, dir: &Path) -> (String, String) {
    let mut name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("skill")
        .to_string();
    let mut description = String::new();

    if let Some(rest) = text.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                if let Some(v) = line.strip_prefix("name:") {
                    name = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("description:") {
                    description = v.trim().to_string();
                }
            }
        }
    }
    if description.is_empty() {
        description = text
            .lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("---"))
            .unwrap_or("")
            .chars()
            .take(200)
            .collect();
    }
    (name, description)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_obtain_oid() {
        // runs from crate root; the skill exists in the repo
        let skills = discover("skills");
        let oid = skills.iter().find(|s| s.name == "obtain-oid");
        assert!(oid.is_some(), "obtain-oid skill should be discovered");
        let oid = oid.unwrap();
        assert!(!oid.description.is_empty());
        assert!(oid.files.iter().any(|f| f == "pull_oid.sh"), "should list its tooling");
    }
}
