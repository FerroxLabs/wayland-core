//! F.1 — locked-variant-count guard + serde roundtrip for every
//! `CuaOp` and `CuaOpResult` variant. Bumping the variant counts
//! requires re-auditing design §5.18 (no `Drag` in v1 etc.).

use wcore_cua::{CUA_OP_LOCKED_VARIANT_COUNT, CuaOp};

#[test]
fn op_count_matches_locked_constant() {
    let variants = CuaOp::all_variants_for_test();
    assert_eq!(
        variants.len(),
        CUA_OP_LOCKED_VARIANT_COUNT,
        "CuaOp variant count drifted from the locked constant — design §5.18 audit required"
    );
}

#[test]
fn op_serde_roundtrip_every_variant() {
    for op in CuaOp::all_variants_for_test() {
        let s = serde_json::to_string(&op).expect("op serializes");
        let back: CuaOp = serde_json::from_str(&s).expect("op deserializes");
        assert_eq!(op, back, "roundtrip failed for {s}");
    }
}

#[test]
fn op_kind_tags_are_unique_and_snake_case() {
    let mut tags = Vec::new();
    for op in CuaOp::all_variants_for_test() {
        let tag = op.kind_tag();
        assert!(
            tag.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "kind tag {tag:?} is not snake_case"
        );
        tags.push(tag);
    }
    let unique: std::collections::HashSet<_> = tags.iter().copied().collect();
    assert_eq!(unique.len(), tags.len(), "duplicate kind tags: {tags:?}");
}

#[test]
fn op_serializes_with_kind_tag() {
    let op = CuaOp::Wait { duration_ms: 100 };
    let v = serde_json::to_value(&op).unwrap();
    assert_eq!(v["kind"], "wait");
    assert_eq!(v["duration_ms"], 100);
}

#[test]
fn no_drag_variant_per_design_5_18() {
    // Sanity: walk the variant list and make sure no variant kind tag
    // contains "drag" — drag-and-drop is intentionally absent in v1.
    for op in CuaOp::all_variants_for_test() {
        let tag = op.kind_tag();
        assert!(
            !tag.contains("drag"),
            "v1 forbids drag operations (design §5.18 background invariant)"
        );
    }
}
