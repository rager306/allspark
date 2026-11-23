// The `loom` feature selects the memory-model lane (matrix row MUT-loom-spinlock).
//
// A cargo FEATURE cannot turn on a `cfg` by itself, and loom's model is selected
// by `--cfg loom`. Without this file, `--features loom` compiles the dependency
// but leaves every `#[cfg(loom)]` item out of the build, so the lane runs zero
// tests and reports green — the "green lane that proves nothing" this repository
// treats as a defect. Emitting the cfg here keeps one knob: the feature.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Declared unconditionally so a plain `cargo check` (no feature) does not warn
    // about an unknown cfg in the source it still parses.
    println!("cargo::rustc-check-cfg=cfg(loom)");
    if std::env::var_os("CARGO_FEATURE_LOOM").is_some() {
        println!("cargo:rustc-cfg=loom");
    }
}
