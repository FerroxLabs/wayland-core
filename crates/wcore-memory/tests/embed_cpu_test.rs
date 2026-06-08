// W5 Task A.6 acceptance: embedding produces 384-dim vector, self-cosine 1.0,
// is deterministic, and related texts cosine higher than unrelated.

use wcore_memory::embed::{
    EMBEDDING_DIM, Embedder, HashedEmbedder, cosine, decode_blob, encode_blob,
};

#[tokio::test]
async fn embedding_is_384_dims_and_self_cosine_is_one() {
    let e = HashedEmbedder::new().await.unwrap();
    let v = e.embed("hello rust world").await.unwrap();
    assert_eq!(v.len(), EMBEDDING_DIM);
    assert_eq!(e.dim(), EMBEDDING_DIM);
    let c = cosine(&v, &v);
    assert!((c - 1.0).abs() < 1e-5, "self cosine {c}");
}

#[tokio::test]
async fn embedding_is_deterministic_within_session() {
    let e = HashedEmbedder::new().await.unwrap();
    let a = e.embed("deterministic input string").await.unwrap();
    let b = e.embed("deterministic input string").await.unwrap();
    assert_eq!(a, b, "embedding should be bit-deterministic");
}

#[tokio::test]
async fn related_better_than_unrelated() {
    let e = HashedEmbedder::new().await.unwrap();
    let q = e.embed("rust async tokio runtime").await.unwrap();
    let near = e.embed("rust async runtime tokio").await.unwrap();
    let far = e.embed("javascript browser dom").await.unwrap();
    assert!(cosine(&q, &near) > cosine(&q, &far));
}

#[test]
fn blob_roundtrip_preserves_vector() {
    let v: Vec<f32> = (0..EMBEDDING_DIM).map(|i| (i as f32) * 0.1).collect();
    let b = encode_blob(&v);
    let back = decode_blob(&b).unwrap();
    assert_eq!(v, back);
}
