//! Live rust-analyzer smoke: the resolved toolchain binary completes the
//! initialize handshake, which is the step that failed with the rustup shim.
//!
//! Ignored by default: it spawns a real language server. Run with:
//!
//! ```text
//! PA_LSP_LIVE=1 cargo test -p pa-lsp --test live_rust -- --ignored --nocapture
//! ```

#[tokio::test]
#[ignore = "spawns rust-analyzer; run with PA_LSP_LIVE=1 --ignored"]
async fn the_resolved_rust_analyzer_completes_the_handshake() {
    if std::env::var_os("PA_LSP_LIVE").is_none() {
        eprintln!("skipping: set PA_LSP_LIVE=1 to run the live rust-analyzer test");
        return;
    }
    let server = pa_lsp::builtin_servers()
        .into_iter()
        .find(|server| server.name == "rust-analyzer")
        .expect("rust-analyzer entry");
    eprintln!(
        "rust-analyzer command: {} (exists: {})",
        server.command,
        std::path::Path::new(&server.command).is_file()
    );
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("src dir");
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"pa_lsp_live\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("Cargo.toml");
    let file = dir.path().join("src/lib.rs");
    std::fs::write(&file, "pub fn target() {}\npub fn caller() { target(); }\n").expect("lib.rs");
    let root = pa_lsp::project_root(&file, &server);
    let client = pa_lsp::LspClient::spawn(&server, &root)
        .await
        .expect("the resolved rust-analyzer completes initialize");
    client.ensure_open(&file).await.expect("didOpen");
}
