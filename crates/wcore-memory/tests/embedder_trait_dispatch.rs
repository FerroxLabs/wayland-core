// M4.5 — Embedder is now a trait; verify dynamic dispatch + that the
// shipped semantic-recall path still accepts the trait-object form.

use std::sync::Arc;

use wcore_memory::embed::{Embedder, HashedEmbedder};

#[tokio::test]
async fn hashed_embedder_dispatches_as_trait_object() {
    let h: Arc<dyn Embedder> = Arc::new(HashedEmbedder::new().await.unwrap());
    let v = h.embed("hello world").await.unwrap();
    assert_eq!(v.len(), 384);
    assert_eq!(h.dim(), 384);
    assert_eq!(h.name(), "hashed/384");
}

#[tokio::test]
async fn trait_object_clones_via_arc() {
    // Cheap to share across tokio tasks — every consumer site holds
    // Arc<dyn Embedder>. The Arc is what clones, not the underlying
    // backend; this guards against accidental deep-clone regressions
    // when real backends land.
    let h: Arc<dyn Embedder> = Arc::new(HashedEmbedder::new().await.unwrap());
    let h2 = Arc::clone(&h);
    let a = h.embed("same input").await.unwrap();
    let b = h2.embed("same input").await.unwrap();
    assert_eq!(a, b);
}

#[tokio::test]
async fn semantic_partition_accepts_trait_object() {
    // Smoke: SemanticPartition::new now takes Arc<dyn Embedder>. This is
    // a compile-time gate — if a future refactor regresses the constructor
    // type, this test stops compiling.
    use std::sync::Arc;
    use wcore_memory::cdc::CdcWriter;
    use wcore_memory::db::Db;
    use wcore_memory::partition::SemanticPartition;

    let db = Arc::new(Db::open_memory().unwrap());
    let embedder: Arc<dyn Embedder> = Arc::new(HashedEmbedder::new().await.unwrap());
    let cdc = Arc::new(CdcWriter::new_stub());
    let _p = SemanticPartition::new(db, embedder, cdc);
}
