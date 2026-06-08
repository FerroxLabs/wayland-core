use std::path::PathBuf;

/// Errors that can occur within the memory system.
///
/// Variants cover both v1 (YAML store) and v2 (SQLite cognitive memory) paths.
/// v1 variants are kept while the v1 surface remains; they will be removed at
/// Group G when wcore-agent cuts over to the v2 API.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    // ----- v1 (YAML flat-file store) -----
    /// File I/O error.
    #[error("memory I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// YAML frontmatter failed to parse.
    #[error("failed to parse frontmatter in {path}: {source}")]
    FrontmatterParse {
        path: PathBuf,
        source: serde_yaml::Error,
    },

    /// Memory path failed security validation.
    #[error("path validation failed: {0}")]
    PathValidation(String),

    // ----- v2 (SQLite cognitive memory) -----
    /// rusqlite-level error (DB open, migration, statement).
    #[error("memory DB: {0}")]
    Db(#[from] rusqlite::Error),

    /// Access gate refused a read/write.
    #[error("memory access denied: partition={partition} tier={tier} reason={reason}")]
    AccessDenied {
        partition: String,
        tier: String,
        reason: String,
    },

    /// Embedding pipeline (tokenize/forward/normalize) failed.
    #[error("embedding error: {0}")]
    Embedding(String),

    /// Schema migration failed at a specific version.
    #[error("migration error at v{version}: {source}")]
    Migration {
        version: u32,
        #[source]
        source: rusqlite::Error,
    },

    /// ConsolidationEngine (dream cycle) failed.
    #[error("memory consolidation: {0}")]
    Consolidation(String),

    /// Letta conversation-window compaction failed.
    #[error("memory compaction: {0}")]
    Compaction(String),

    /// One-shot YAML → P2 importer failed.
    #[error("legacy import: {0}")]
    LegacyImport(String),

    /// CDC changelog write failed.
    #[error("cdc changelog: {0}")]
    Cdc(String),

    /// Audit-log write failed.
    #[error("audit log: {0}")]
    Audit(String),

    /// M5.7 — lineage edge would create a cycle in the swarm parent/child
    /// session graph (or self-edge). The payload describes the offending
    /// edge for operator logs.
    #[error("lineage cycle: {0}")]
    LineageCycle(String),

    /// M5.7 — refused to read a target session that is downstream of the
    /// reader in the lineage DAG (cycle / direction guard for the
    /// SwarmMemoryBridge).
    #[error("descendant read denied: {0}")]
    DescendantReadDenied(String),
}

pub type Result<T> = std::result::Result<T, MemoryError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_display() {
        let inner = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err = MemoryError::Io(inner);
        let msg = err.to_string();
        assert!(msg.contains("I/O"), "should mention I/O: {msg}");
        assert!(msg.contains("gone"), "should contain inner message: {msg}");
    }

    #[test]
    fn io_error_from_conversion() {
        let inner = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err: MemoryError = inner.into();
        assert!(matches!(err, MemoryError::Io(_)));
    }

    #[test]
    fn path_validation_display() {
        let err = MemoryError::PathValidation("relative path".into());
        let msg = err.to_string();
        assert!(
            msg.contains("relative path"),
            "should contain reason: {msg}"
        );
        assert!(
            msg.contains("validation"),
            "should mention validation: {msg}"
        );
    }

    #[test]
    fn frontmatter_parse_display() {
        // Trigger a real serde_yaml error
        let yaml_err = serde_yaml::from_str::<serde_yaml::Value>(":\n  :\n---").unwrap_err();
        let err = MemoryError::FrontmatterParse {
            path: PathBuf::from("/tmp/test.md"),
            source: yaml_err,
        };
        let msg = err.to_string();
        assert!(msg.contains("/tmp/test.md"), "should contain path: {msg}");
        assert!(
            msg.contains("frontmatter"),
            "should mention frontmatter: {msg}"
        );
    }

    // ----- v2 variant tests (A.2) -----

    #[test]
    fn db_error_displays() {
        let inner = rusqlite::Error::InvalidQuery;
        let err: MemoryError = inner.into();
        let msg = err.to_string();
        assert!(msg.contains("memory DB"), "should mention DB: {msg}");
    }

    #[test]
    fn token_denied_includes_partition_and_tier() {
        let err = MemoryError::AccessDenied {
            partition: "core".into(),
            tier: "global".into(),
            reason: "system-only write".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("core"), "should mention partition: {msg}");
        assert!(msg.contains("global"), "should mention tier: {msg}");
        assert!(msg.contains("system-only"), "should mention reason: {msg}");
    }

    #[test]
    fn embedding_error_carries_source() {
        let err = MemoryError::Embedding("tokenizer failed".into());
        let msg = err.to_string();
        assert!(msg.contains("embedding"), "should mention embedding: {msg}");
        assert!(msg.contains("tokenizer"), "should carry source: {msg}");
    }

    #[test]
    fn migration_error_carries_version() {
        let err = MemoryError::Migration {
            version: 1,
            source: rusqlite::Error::InvalidQuery,
        };
        let msg = err.to_string();
        assert!(msg.contains("v1"), "should mention version: {msg}");
        assert!(msg.contains("migration"), "should mention migration: {msg}");
    }

    #[test]
    fn consolidation_error_display() {
        let err = MemoryError::Consolidation("dream cycle stalled".into());
        assert!(err.to_string().contains("consolidation"));
    }

    #[test]
    fn compaction_error_display() {
        let err = MemoryError::Compaction("summarizer unavailable".into());
        assert!(err.to_string().contains("compaction"));
    }

    #[test]
    fn legacy_import_error_display() {
        let err = MemoryError::LegacyImport("bad frontmatter".into());
        assert!(err.to_string().contains("legacy"));
    }

    #[test]
    fn cdc_error_display() {
        let err = MemoryError::Cdc("file locked".into());
        assert!(err.to_string().contains("cdc"));
    }

    #[test]
    fn audit_error_display() {
        let err = MemoryError::Audit("write failed".into());
        assert!(err.to_string().contains("audit"));
    }
}
