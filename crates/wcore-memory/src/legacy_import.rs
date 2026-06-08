// One-shot importer: v1 YAML-frontmatter `.md` memory files → v2 P2 Episodic.
//
// Idempotency: a row in `legacy_import_marker` keyed by the absolute YAML
// directory prevents re-imports. The original `.md` files are NEVER
// deleted — the user can verify before opting in to a cleanup wave.
//
// Wired by Group A; Group F (CDC) hooks the per-episode + bulk markers
// into the changelog.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::Db;
use crate::embed::{Embedder, encode_blob};
use crate::error::{MemoryError, Result};
use crate::v2_types::{Episode, EpisodeId, EpisodeStatus, Tier};

/// Report of a single import call.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LegacyImportReport {
    pub yaml_dir: PathBuf,
    pub episodes_inserted: usize,
    pub already_imported: bool,
}

/// If `yaml_dir` exists and isn't yet imported, walk every `*.md` file
/// (except MEMORY.md), parse YAML frontmatter, and insert one P2 episode
/// per file into the **global** tier. Returns the report.
///
/// Re-running this function on the same `yaml_dir` is a no-op: it returns
/// `already_imported = true`.
pub async fn import_if_present(
    db: &Db,
    embedder: &dyn Embedder,
    yaml_dir: &Path,
) -> Result<LegacyImportReport> {
    let mut report = LegacyImportReport {
        yaml_dir: yaml_dir.to_path_buf(),
        ..Default::default()
    };

    if !yaml_dir.exists() {
        return Ok(report);
    }

    let marker_key = yaml_dir.to_string_lossy().into_owned();
    if marker_present(db, &marker_key)? {
        report.already_imported = true;
        return Ok(report);
    }

    let files = collect_md_files(yaml_dir)?;
    for path in files {
        let body = std::fs::read_to_string(&path)
            .map_err(|e| MemoryError::LegacyImport(format!("read {path:?}: {e}")))?;
        let parsed = parse_frontmatter(&body, &path)?;
        let ep = build_episode(&parsed);
        let embedding = embedder.embed(&ep.summary).await?;
        insert_episode(db, Tier::Global, &ep, &embedding)?;
        report.episodes_inserted += 1;
    }

    write_marker(db, &marker_key, report.episodes_inserted)?;
    Ok(report)
}

fn marker_present(db: &Db, key: &str) -> Result<bool> {
    let tc = db.global.clone();
    let conn = tc.conn.lock();
    let mut stmt =
        conn.prepare("SELECT 1 FROM legacy_import_marker WHERE yaml_dir = ?1 LIMIT 1")?;
    let exists: rusqlite::Result<i64> = stmt.query_row([key], |r| r.get(0));
    match exists {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(e) => Err(MemoryError::Db(e)),
    }
}

fn write_marker(db: &Db, key: &str, count: usize) -> Result<()> {
    let tc = db.global.clone();
    let conn = tc.conn.lock();
    let ts = now_secs();
    conn.execute(
        "INSERT OR REPLACE INTO legacy_import_marker (yaml_dir, imported_at, episode_count) VALUES (?1, ?2, ?3)",
        rusqlite::params![key, ts, count as i64],
    )?;
    Ok(())
}

fn collect_md_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(dir)
        .map_err(|e| MemoryError::LegacyImport(format!("read_dir {dir:?}: {e}")))?;
    for entry in entries {
        let entry = entry.map_err(MemoryError::Io)?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if name == crate::paths::ENTRYPOINT_NAME {
            continue;
        }
        if !name.ends_with(".md") {
            continue;
        }
        out.push(path);
    }
    out.sort(); // deterministic ordering
    Ok(out)
}

#[derive(Debug)]
struct ParsedYaml {
    title: String,
    summary: String,
    body: String,
    type_hint: Option<String>,
}

fn parse_frontmatter(body: &str, path: &Path) -> Result<ParsedYaml> {
    let (fm_str, body_str) = split_frontmatter(body);
    let mut title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "untitled".into());
    let mut type_hint = None;

    if let Some(fm) = fm_str {
        // Best-effort YAML parsing — we only need `title` and `type`.
        if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(fm) {
            if let Some(t) = value.get("title").and_then(|v| v.as_str()) {
                title = t.to_string();
            }
            if let Some(t) = value.get("type").and_then(|v| v.as_str()) {
                type_hint = Some(t.to_string());
            }
        }
    }

    let summary = body_str
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| title.clone());

    Ok(ParsedYaml {
        title,
        summary,
        body: body_str.to_string(),
        type_hint,
    })
}

/// Split `---\nfrontmatter\n---\nbody` into (frontmatter, body). If no
/// frontmatter delimiters are present, returns (None, whole_input).
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    if let Some(rest) = text.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let fm = &rest[..end];
            let body = &rest[end + 5..];
            return (Some(fm), body);
        }
        if let Some(end) = rest.find("\n---") {
            let fm = &rest[..end];
            let body = rest.get(end + 4..).unwrap_or("");
            return (Some(fm), body);
        }
    }
    (None, text)
}

fn build_episode(p: &ParsedYaml) -> Episode {
    let body_preview: String = p.body.lines().take(10).collect::<Vec<_>>().join("\n");
    let _ = &p.title; // included via summary fallback
    let _ = body_preview;
    Episode {
        id: EpisodeId::new(),
        tier: Tier::Global,
        ts: now_secs(),
        episode_type: p
            .type_hint
            .clone()
            .unwrap_or_else(|| "legacy_yaml".to_string()),
        summary: p.summary.clone(),
        atomic_facts: Vec::new(),
        source: "legacy".to_string(),
        source_product: "wcore-memory-v1".to_string(),
        session_id: None,
        project_root: None,
        decay_score: 1.0,
        status: EpisodeStatus::Active,
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Low-level insert used by the importer. Mirrors what the dispatcher's
/// `record_episode` path will use (Group C); centralised here so legacy
/// import can pre-date the dispatcher.
pub fn insert_episode(db: &Db, tier: Tier, ep: &Episode, embedding: &[f32]) -> Result<()> {
    let tc = db.tier_or_global(tier);
    let conn = tc.conn.lock();
    let atomic_json = serde_json::to_string(&ep.atomic_facts).unwrap_or_else(|_| "[]".into());
    let blob = encode_blob(embedding);
    conn.execute(
        "INSERT INTO episodes (id, tier, ts, episode_type, summary, atomic_facts, source, source_product, session_id, project_root, decay_score, status, embedding)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            ep.id.0.to_string(),
            ep.tier.as_str(),
            ep.ts,
            ep.episode_type,
            ep.summary,
            atomic_json,
            ep.source,
            ep.source_product,
            ep.session_id,
            ep.project_root,
            ep.decay_score,
            ep.status.as_str(),
            blob,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_basic() {
        let s = "---\ntitle: x\n---\nbody here";
        let (fm, body) = split_frontmatter(s);
        assert_eq!(fm, Some("title: x"));
        assert_eq!(body, "body here");
    }

    #[test]
    fn split_frontmatter_none() {
        let s = "just a body";
        let (fm, body) = split_frontmatter(s);
        assert_eq!(fm, None);
        assert_eq!(body, "just a body");
    }
}
