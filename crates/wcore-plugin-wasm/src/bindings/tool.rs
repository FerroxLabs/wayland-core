//! Tool-world bindings (Task 2.2).
//!
//! Generates the `wayland-tool` world bindings from `wit/tool.wit`
//! (and its shared host import from `wit/wayland-host.wit`).
//! `async: true` because Tool `execute` runs on Tokio.
wasmtime::component::bindgen!({
    path: "wit",
    world: "wayland-tool",
    imports: { default: async },
    exports: { default: async },
});
