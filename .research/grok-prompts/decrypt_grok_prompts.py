#!/usr/bin/env python3
"""Decrypt Grok Build's XOR-obfuscated prompt templates.

Scheme (crates/codegen/xai-grok-agent/src/prompt/template.rs:14):
    plain[i] = enc[i] ^ (seed + i) mod 256
Seeds live in-repo: prompt_encrypted.rs:14 PROMPT_SEEDS = [0x5A, 0x7B, 0x3D]

This is obfuscation, not security. Plaintext .md files also ship in
crates/codegen/xai-grok-agent/templates/, so decryption is only needed to
verify the blobs are not stale.

Usage: python3 decrypt_grok_prompts.py /tmp/grok-build [outdir]
"""
import os, re, sys

SEEDS = [0x5A, 0x7B, 0x3D]
NAMES = ["BASE_PROMPT_ENC", "CODEX_PROMPT_ENC", "SUBAGENT_PROMPT_ENC"]
FILES = ["prompt.md", "apply_patch_prompt.md", "subagent_prompt.md"]


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else "/tmp/grok-build"
    out = sys.argv[2] if len(sys.argv) > 2 else "/tmp/roadmap_audit/grok_prompts"
    os.makedirs(out, exist_ok=True)

    enc_rs = os.path.join(root, "crates/codegen/xai-grok-agent/src/prompt/prompt_encrypted.rs")
    src = open(enc_rs).read()

    arrays = {}
    for m in re.finditer(r"const (\w+): &\[u8\] = &\[([^\]]*)\]", src, re.S):
        arrays[m.group(1)] = bytes(
            int(x) for x in m.group(2).replace("\n", " ").split(",") if x.strip()
        )

    tmpl_dir = os.path.join(root, "crates/codegen/xai-grok-agent/templates")
    for name, fname, seed in zip(NAMES, FILES, SEEDS):
        data = arrays[name]
        plain = bytes(b ^ ((seed + i) & 0xFF) for i, b in enumerate(data)).decode("utf-8")
        dst = os.path.join(out, fname)
        open(dst, "w").write(plain)
        ref_path = os.path.join(tmpl_dir, fname)
        if os.path.exists(ref_path):
            same = plain == open(ref_path).read()
            print(f"{fname:26s} {len(plain):6d} chars  match_repo={same}  -> {dst}")
        else:
            print(f"{fname:26s} {len(plain):6d} chars  (no repo plaintext)  -> {dst}")


if __name__ == "__main__":
    main()
