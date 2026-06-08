//! Hook-world bindings (Task 2.3).
//!
//! Generated from `wit/hook.wit`. The world `wayland-hook` imports the shared
//! `wayland:host/host` interface and exports the `hook` interface.

wasmtime::component::bindgen!({
    path: "wit",
    world: "wayland-hook",
    imports: { default: async },
    exports: { default: async },
});
