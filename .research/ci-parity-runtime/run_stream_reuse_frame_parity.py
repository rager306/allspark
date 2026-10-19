from pathlib import Path
import contextlib, hashlib, io, json, os, sys, tempfile
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import review_markdown_frames_parity as verifier
RUNTIME = ROOT / ".research/ci-parity-runtime"
RECEIPT = ROOT / ".research/ci-stream-frame-reuse-parity.json"
verifier.CORPUS = {
    "h": "HTTPS://example.com",
    "f": "ftp://example.com",
    "w": "www.example.com",
    "e": "agent@example.com",
    "u": "Unicode café 界 https://example.com",
    "x": "[https://example.com](https://example.com)",
}
os.environ.update(PA_TS_BINARY=str(ROOT / ".research/ci-tty-ts-reference/package/prime-agent"),
                  PA_RUST_BINARY=str(ROOT / "target/debug/prime-agent"),
                  PA_TS_ARCHIVE=str(ROOT / ".research/ci-tty-ts-reference/prime-agent-0.9.8-linux-x64.tar.gz"),
                  PI_PACKAGE_DIR=str(ROOT / ".research/ci-tty-ts-reference/package"))
original_tempdir = tempfile.TemporaryDirectory
def repo_tempdir(*args, **kwargs):
    kwargs.update(dir=str(RUNTIME), prefix="m-")
    return original_tempdir(*args, **kwargs)
tempfile.TemporaryDirectory = repo_tempdir
original_write = Path.write_text
def hashed(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
def sanitize(result):
    out = {k: result[k] for k in ("verified", "scope", "source_commit", "ts_archive_sha256", "comparisons") if k in result}
    out["wrapper_scope"] = "Existing real-binary Markdown frame verifier; selected six autolink/Unicode/explicit-link inputs at unchanged40/80/120 widths. No verifier source edits. Sanitized styled-region hashes and geometry only; excludes raw frames, commands, prompts, environment, runtime artifacts."
    if "error" in result:
        out["error"] = {"sha256":hashed(result["error"]), "characters":len(result["error"])}
    out["ts_reference_source_commit"] = out.pop("source_commit", None)
    out["rust_base_commit"] = "8f64f3138"
    out["rust_source_sha256"] = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                                 for p in (ROOT / "crates/pa-tui/src/view/lazy.rs", ROOT / "crates/pa-tui/src/view/lazy_tests.rs")}
    out["results"] = {}
    for kind, side in result.get("results", {}).items():
        cases = {}
        for key, case in side.get("cases", {}).items():
            region = case.get("styled_region", [])
            cases[key] = {"executed":case.get("executed",False), "styled_region_sha256":hashed(region),
                          "row_count":len(region), "cell_count":sum(len(row) for row in region),
                          "osc8_observed":case.get("osc8_observed",False)}
            if "error" in case:
                cases[key]["error"] = {"sha256":hashed(case["error"]), "characters":len(case["error"])}
        out["results"][kind] = {"binary_sha256":side.get("binary_sha256"), "cases":cases}
    return out
def safe_write(path, text, *args, **kwargs):
    if path.resolve() == RECEIPT:
        text = json.dumps(sanitize(json.loads(text)), indent=2, sort_keys=True) + "\n"
    return original_write(path, text, *args, **kwargs)
Path.write_text = safe_write
sys.argv = [str(ROOT / "scripts/review_markdown_frames_parity.py"), "--receipt", str(RECEIPT)]
with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
    status = verifier.main()
result = json.loads(RECEIPT.read_text())
print(json.dumps({"verifier":"review_markdown_frames_parity.py", "exit_code":status,
                  "verified":result["verified"], "comparison_count":len(result.get("comparisons",{})),
                  "matched":sum(result.get("comparisons",{}).values()), "error":result.get("error")}))
sys.exit(status)
