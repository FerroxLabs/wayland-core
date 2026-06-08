//! Validate hook WIT syntax (Task 2.3).
//!
//! Resolves the whole `wit/` directory because `hook.wit` shares its
//! `wayland:host@1.0.0` package decl with `wayland-host.wit` (single package
//! decl rule, see wit-parser docs).
#[test]
fn hook_wit_parses() {
    use wit_parser::Resolve;
    let mut r = Resolve::new();
    let _ = r.push_path("wit").expect("hook WIT parses");
}
