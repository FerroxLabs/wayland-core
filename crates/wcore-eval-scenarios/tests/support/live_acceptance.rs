//! Shared live-test acceptance predicates. The initial W01 checkpoint preserves
//! the existing predicate so its regression controls can prove the false pass.

pub fn successful_read_answer(
    text: &str,
    sentinel: &str,
    turns: usize,
    _read_succeeded: bool,
) -> bool {
    !text.is_empty() && (text.contains(sentinel) || turns > 1)
}
