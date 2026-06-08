//! Sanity check that wcore-eval is reachable as a workspace member.
#[test]
fn crate_compiles_and_re_exports_error_type() {
    let _: wcore_eval::EvalError = wcore_eval::EvalError::CorpusEmpty;
}
