use std::time::Instant;
#[test]
#[ignore = "measurement"]
fn head_read_cost() {
    let cwd = std::path::Path::new("/root/prime-agent");
    let n = 200;
    let start = Instant::now();
    for _ in 0..n {
        let _ = pa_core::session::manager::capture_git_context(cwd);
    }
    let process = start.elapsed();
    let start = Instant::now();
    for _ in 0..n {
        let _ = pa_core::session::manager::read_head_commit(cwd);
    }
    let file = start.elapsed();
    eprintln!(
        "git probes: {:?}/call, head read: {:?}/call",
        process / n,
        file / n
    );
}
