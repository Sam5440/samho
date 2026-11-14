use crate::object::{InvocationContext, Object, ObjectEnvelope, ObjectKind};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub disable_model_invocation: bool,
    pub allowed_tools: Option<Value>,
    pub metadata: BTreeMap<String, Value>,
    #[serde(default)]
    pub content_sha256: String,
}
#[derive(Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default, rename = "disable-model-invocation")]
    disable_model_invocation: bool,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<Value>,
    #[serde(default)]
    metadata: BTreeMap<String, Value>,
}
fn split_frontmatter(text: &str) -> Result<(&str, &str)> {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.split_inclusive('\n');
    if lines.next().map(str::trim) != Some("---") {
        bail!("SKILL.md needs YAML frontmatter");
    }
    let start = text
        .find('\n')
        .ok_or_else(|| anyhow::anyhow!("missing skill body"))?
        + 1;
    let mut offset = start;
    for line in lines {
        if line.trim() == "---" {
            return Ok((&text[start..offset], &text[offset + line.len()..]));
        }
        offset += line.len();
    }
    bail!("unterminated skill frontmatter")
}
impl Skill {
    pub fn read_metadata(path: &Path, root: &Path) -> Result<Self> {
        reject_symlink_components(path, root)?;
        let text = std::fs::read_to_string(path)?;
        if text.len() > 1024 * 1024 {
            bail!("skill exceeds 1MiB");
        }
        let (yaml, _) = split_frontmatter(&text)?;
        let front: Frontmatter = serde_yaml::from_str(yaml).context("invalid skill YAML")?;
        let name = &front.name;
        if name.is_empty()
            || name.len() > 64
            || name.starts_with('-')
            || name.ends_with('-')
            || name.contains("--")
            || !name
                .bytes()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == b'-')
        {
            bail!("invalid skill name {name}");
        }
        if front.description.trim().is_empty() || front.description.chars().count() > 1024 {
            bail!("invalid skill description");
        }
        if let Some(value) = &front.allowed_tools {
            allowed_tool_patterns(value)?;
        }
        Ok(Self {
            name: front.name,
            description: front.description,
            path: path.to_path_buf(),
            disable_model_invocation: front.disable_model_invocation,
            allowed_tools: front.allowed_tools,
            metadata: front.metadata,
            content_sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
        })
    }
    pub async fn instructions(&self) -> Result<String> {
        // 以技能自身目录为锚点复检文件本体；更上层组件在扫描时已逐级 lstat。
        let dir = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("skill has no directory"))?;
        reject_symlink_components(&self.path, dir)?;
        let text = tokio::fs::read_to_string(&self.path).await?;
        if text.len() > 1024 * 1024 {
            bail!("skill exceeds 1MiB");
        }
        let (_, body) = split_frontmatter(&text)?;
        Ok(format!(
            "Skill: {}\nBase directory: {}\n\n{}",
            self.name,
            self.path.parent().unwrap_or(Path::new(".")).display(),
            body.trim()
        ))
    }
    /// Read an inert resource below the skill directory. This never executes a script.
    pub async fn read_resource(&self, relative: &str, max_bytes: usize) -> Result<Value> {
        use tokio::io::AsyncReadExt;
        let relative = Path::new(relative);
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative.components().any(|c| {
                !matches!(
                    c,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        {
            bail!("skill resource must be a relative path without traversal");
        }
        let root = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("skill has no directory"))?;
        reject_symlink_components(root, root)?;
        let root = std::fs::canonicalize(root)?;
        let candidate = root.join(relative);
        reject_symlink_components(&candidate, &root)?;
        let candidate = std::fs::canonicalize(candidate)?;
        if !candidate.starts_with(&root) || !candidate.is_file() {
            bail!("skill resource escapes its directory or is not a file");
        }
        let mut bytes = Vec::new();
        tokio::fs::File::open(&candidate)
            .await?
            .take(max_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > max_bytes {
            bail!("skill resource exceeds configured limit");
        }
        let text = String::from_utf8(bytes).context("skill resource is not UTF-8")?;
        Ok(
            json!({"skill":self.name,"path":relative.to_string_lossy(),"content":text,"sha256":format!("{:x}",Sha256::digest(text.as_bytes()))}),
        )
    }
    pub fn permits_tool(&self, name: &str) -> Result<bool> {
        let Some(value) = &self.allowed_tools else {
            return Ok(true);
        };
        let patterns = allowed_tool_patterns(value)?;
        let name = name.strip_prefix("tool:").unwrap_or(name);
        Ok(patterns
            .iter()
            .any(|pattern| glob_tool_match(pattern, name)))
    }
}
fn reject_symlink_components(path: &Path, anchor: &Path) -> Result<()> {
    // 只检查受管根（anchor）之下的组件：anchor 本身与其到 path 的相对部分
    // 逐组件 lstat。系统级祖先（macOS 的 /var、/tmp 是指向 private/* 的符号
    // 链接）不属于 skill 管理范围，不参与检查，否则会把合法路径误判为逃逸；
    // 根内部的链接逃逸防护保持完全严格。
    let relative = path
        .strip_prefix(anchor)
        .map_err(|_| anyhow::anyhow!("skill path {} escapes its managed root", path.display()))?;
    let mut current = anchor.to_path_buf();
    if std::fs::symlink_metadata(&current)?
        .file_type()
        .is_symlink()
    {
        bail!("symlink skill paths are not allowed");
    }
    for component in relative.components() {
        current.push(component.as_os_str());
        if std::fs::symlink_metadata(&current)?
            .file_type()
            .is_symlink()
        {
            bail!("symlink skill paths are not allowed");
        }
    }
    Ok(())
}
fn allowed_tool_patterns(value: &Value) -> Result<Vec<String>> {
    let patterns = match value {
        Value::String(text) => text.split_whitespace().map(str::to_owned).collect(),
        Value::Array(values) => values
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow::anyhow!("allowed-tools must contain strings"))
            })
            .collect::<Result<Vec<_>>>()?,
        _ => bail!("allowed-tools must be a whitespace-separated string or string array"),
    };
    for pattern in &patterns {
        if pattern.is_empty()
            || !pattern
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-:*?".contains(&byte))
        {
            bail!("invalid allowed-tools pattern {pattern}");
        }
    }
    Ok(patterns)
}
fn glob_tool_match(pattern: &str, tool: &str) -> bool {
    let pattern = pattern.strip_prefix("tool:").unwrap_or(pattern);
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
    }
    regex.push('$');
    regex::Regex::new(&regex).is_ok_and(|regex| regex.is_match(tool))
}
#[async_trait]
impl Object for Skill {
    fn envelope(&self) -> ObjectEnvelope {
        let mut descriptor = ObjectEnvelope::new(
            format!("skill:{}", self.name),
            ObjectKind::Skill,
            &self.description,
        );
        descriptor.spec = serde_json::to_value(self).expect("skill metadata");
        descriptor.capabilities.insert(
            "load".into(),
            json!({"type":"object","properties":{},"additionalProperties":false}),
        );
        descriptor
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        if context.cancel.is_cancelled() {
            bail!("cancelled");
        }
        match method {
            "load" => Ok(json!({"instructions":self.instructions().await?})),
            "inspect" => Ok(serde_json::to_value(self)?),
            "read_resource" => {
                self.read_resource(
                    input["path"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("resource path is required"))?,
                    input["max_bytes"]
                        .as_u64()
                        .unwrap_or(64 * 1024)
                        .min(1024 * 1024) as usize,
                )
                .await
            }
            _ => bail!("unknown skill method {method}"),
        }
    }
}
#[derive(Clone, Default)]
pub struct SkillRegistry {
    roots: Arc<Vec<PathBuf>>,
    state: Arc<RwLock<SkillSnapshot>>,
}
#[derive(Default)]
struct SkillSnapshot {
    skills: BTreeMap<String, Skill>,
    diagnostics: Vec<String>,
    generation: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct SkillRefresh {
    pub generation: u64,
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    pub diagnostics: Vec<String>,
}
impl SkillRegistry {
    pub fn discover(roots: &[PathBuf]) -> Result<Self> {
        let mut skills = BTreeMap::new();
        let mut diagnostics = Vec::new();
        fn scan(
            path: &Path,
            depth: u32,
            root: &Path,
            skills: &mut BTreeMap<String, Skill>,
            diagnostics: &mut Vec<String>,
            ignores: &[Gitignore],
        ) {
            if depth > 32 {
                diagnostics.push(format!("skill nesting too deep: {}", path.display()));
                return;
            }
            let Ok(meta) = std::fs::symlink_metadata(path) else {
                return;
            };
            if meta.file_type().is_symlink() {
                return;
            }
            let ignored = ignores.iter().fold(false, |previous, ignore| {
                let matched = ignore.matched_path_or_any_parents(path, meta.is_dir());
                if matched.is_ignore() {
                    true
                } else if matched.is_whitelist() {
                    false
                } else {
                    previous
                }
            });
            if ignored {
                return;
            }
            if meta.is_file() {
                if path.file_name().is_none_or(|name| name != "SKILL.md") {
                    return;
                }
                match Skill::read_metadata(path, root) {
                    Ok(skill) => {
                        if skills.contains_key(&skill.name) {
                            diagnostics.push(format!(
                                "duplicate skill {} ignored: {}",
                                skill.name,
                                path.display()
                            ));
                        } else {
                            skills.insert(skill.name.clone(), skill);
                        }
                    }
                    Err(error) => diagnostics.push(format!("{}: {error}", path.display())),
                }
                return;
            }
            let Ok(entries) = std::fs::read_dir(path) else {
                return;
            };
            let mut ignores = ignores.to_vec();
            let ignore_path = path.join(".skillignore");
            if ignore_path.is_file() {
                let mut builder = GitignoreBuilder::new(path);
                if let Some(error) = builder.add(&ignore_path) {
                    diagnostics.push(format!("{}: {error}", ignore_path.display()));
                }
                match builder.build() {
                    Ok(ignore) => ignores.push(ignore),
                    Err(error) => diagnostics.push(format!("{}: {error}", ignore_path.display())),
                }
            }
            let mut entries = entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .collect::<Vec<_>>();
            entries.sort();
            for path in entries {
                if path.file_name().is_some_and(|name| {
                    [".git", "node_modules", "target"]
                        .iter()
                        .any(|skip| name == *skip)
                }) {
                    continue;
                }
                scan(&path, depth + 1, root, skills, diagnostics, &ignores);
            }
        }
        for root in roots {
            scan(root, 0, root, &mut skills, &mut diagnostics, &[]);
        }
        Ok(Self {
            roots: Arc::new(roots.to_vec()),
            state: Arc::new(RwLock::new(SkillSnapshot {
                skills,
                diagnostics,
                generation: 1,
            })),
        })
    }
    pub fn list(&self) -> Vec<Skill> {
        self.state
            .read()
            .expect("skill registry poisoned")
            .skills
            .values()
            .cloned()
            .collect()
    }
    pub fn diagnostics(&self) -> Vec<String> {
        self.state
            .read()
            .expect("skill registry poisoned")
            .diagnostics
            .clone()
    }
    pub fn get(&self, name: &str) -> Option<Skill> {
        self.state
            .read()
            .expect("skill registry poisoned")
            .skills
            .get(name)
            .cloned()
    }
    /// Atomically publish a complete deterministic discovery generation, visible to all clones.
    pub fn refresh(&self) -> Result<SkillRefresh> {
        let candidate = Self::discover(&self.roots)?;
        let mut next = candidate
            .state
            .write()
            .map_err(|_| anyhow::anyhow!("skill registry poisoned"))?;
        let mut current = self
            .state
            .write()
            .map_err(|_| anyhow::anyhow!("skill registry poisoned"))?;
        let added = next
            .skills
            .keys()
            .filter(|name| !current.skills.contains_key(*name))
            .cloned()
            .collect();
        let removed = current
            .skills
            .keys()
            .filter(|name| !next.skills.contains_key(*name))
            .cloned()
            .collect();
        let changed = next
            .skills
            .iter()
            .filter(|(name, skill)| {
                current.skills.get(*name).is_some_and(|old| {
                    old.content_sha256 != skill.content_sha256 || old.path != skill.path
                })
            })
            .map(|(name, _)| name.clone())
            .collect();
        next.generation = current.generation.saturating_add(1);
        let report = SkillRefresh {
            generation: next.generation,
            added,
            changed,
            removed,
            diagnostics: next.diagnostics.clone(),
        };
        std::mem::swap(&mut *current, &mut *next);
        Ok(report)
    }
    /// All active skills intersect permissions; no metadata field can grant a disabled tool.
    pub fn enforce_allowed_tools(&self, names: &[String], tool: &str) -> Result<()> {
        for name in names {
            let skill = self
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("active skill no longer available: {name}"))?;
            if !skill.permits_tool(tool)? {
                bail!("skill {name} denied tool {tool} by allowed-tools");
            }
        }
        Ok(())
    }
    pub async fn load(&self, name: &str, explicit: bool) -> Result<String> {
        let skill = self
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown skill {name}"))?;
        if skill.disable_model_invocation && !explicit {
            bail!("skill {name} requires explicit user invocation");
        }
        skill.instructions().await
    }
    pub fn objects(&self) -> Vec<Arc<dyn Object>> {
        self.list()
            .into_iter()
            .map(|skill| Arc::new(skill) as Arc<dyn Object>)
            .collect()
    }
}
