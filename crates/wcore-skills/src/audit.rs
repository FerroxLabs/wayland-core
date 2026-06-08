//! F19: one-shot skill-corpus audit. Reports stale, duplicate, broken-ref
//! findings. Output is dual: a Markdown human report and a JSON machine
//! report. The CLI subcommand `wcore skills audit` consumes this (Task 13).

use serde::Serialize;

use crate::refs::SkillRef;

#[derive(Debug, Clone)]
pub struct AuditOpts {
    pub stale_after_days: u64,
    /// Levenshtein distance; descriptions within `<= N` are flagged as dupes.
    pub duplicate_description_distance: u32,
}

impl Default for AuditOpts {
    fn default() -> Self {
        Self {
            stale_after_days: 180,
            duplicate_description_distance: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditReport {
    pub audited_at: String,
    pub total_skills: usize,
    pub findings: Vec<AuditFinding>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum AuditFinding {
    #[serde(rename = "stale")]
    Stale {
        name: String,
        last_modified_days_ago: u64,
    },
    #[serde(rename = "duplicate")]
    Duplicate { skills: Vec<String>, reason: String },
    #[serde(rename = "broken_ref")]
    BrokenRef {
        name: String,
        ref_kind: String,
        target: String,
    },
}

pub fn audit_corpus(refs: &[SkillRef], opts: &AuditOpts) -> AuditReport {
    let now = std::time::SystemTime::now();
    let mut findings: Vec<AuditFinding> = Vec::new();

    // 1. Stale by mtime.
    let stale_cutoff = std::time::Duration::from_secs(opts.stale_after_days * 86_400);
    for r in refs {
        if let Ok(meta) = std::fs::metadata(&r.file_path)
            && let Ok(modified) = meta.modified()
            && let Ok(age) = now.duration_since(modified)
            && age > stale_cutoff
        {
            findings.push(AuditFinding::Stale {
                name: r.name.clone(),
                last_modified_days_ago: age.as_secs() / 86_400,
            });
        }
    }

    // 2. Duplicate descriptions via Levenshtein.
    for (i, a) in refs.iter().enumerate() {
        for b in refs.iter().skip(i + 1) {
            if a.name == b.name {
                continue;
            }
            if a.description.is_empty() {
                continue;
            }
            let d = levenshtein(&a.description, &b.description);
            if d <= opts.duplicate_description_distance as usize {
                findings.push(AuditFinding::Duplicate {
                    skills: vec![a.name.clone(), b.name.clone()],
                    reason: format!("description Levenshtein distance = {d}"),
                });
            }
        }
    }

    // 3. Broken-ref: artifact paths that escape root. Only resolves the
    //    body for skills the loader hinted have artifacts.
    for r in refs {
        if !r.has_artifacts {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&r.file_path) else {
            continue;
        };
        let parsed = crate::frontmatter::parse_frontmatter_with_source(
            &raw,
            Some(&r.file_path.to_string_lossy()),
        );
        let meta = crate::frontmatter::parse_skill_fields(
            &parsed.frontmatter,
            &parsed.content,
            &r.name,
            r.source,
            r.loaded_from,
            r.file_path.parent().and_then(|p| p.to_str()),
        );
        for spec in &meta.artifacts {
            // Path is illegal if it contains `..` or is absolute. Same
            // rule the runtime resolver enforces in artifacts.rs.
            if spec.path.contains("..") || std::path::Path::new(&spec.path).is_absolute() {
                findings.push(AuditFinding::BrokenRef {
                    name: r.name.clone(),
                    ref_kind: "artifact_path".into(),
                    target: spec.path.clone(),
                });
            }
        }
    }

    AuditReport {
        audited_at: chrono::Utc::now().to_rfc3339(),
        total_skills: refs.len(),
        findings,
    }
}

pub fn render_markdown(report: &AuditReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "# Skills Audit ({})", report.audited_at);
    let _ = writeln!(out, "\nTotal skills: {}\n", report.total_skills);
    let _ = writeln!(out, "## Findings ({})", report.findings.len());
    for f in &report.findings {
        match f {
            AuditFinding::Stale {
                name,
                last_modified_days_ago,
            } => {
                let _ = writeln!(
                    out,
                    "- stale `{name}` (last modified {last_modified_days_ago}d ago)"
                );
            }
            AuditFinding::Duplicate { skills, reason } => {
                let _ = writeln!(out, "- duplicate {skills:?} ({reason})");
            }
            AuditFinding::BrokenRef {
                name,
                ref_kind,
                target,
            } => {
                let _ = writeln!(out, "- broken_ref `{name}` ({ref_kind} = {target})");
            }
        }
    }
    out
}

pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ac) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, bc) in b.iter().enumerate() {
            let cost = if ac == bc { 0 } else { 1 };
            cur[j + 1] = (cur[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}
