#!/usr/bin/env python3
"""Doc gate (#268): builds rustdoc for the contract crates and fails on any
warning. soroban-sdk's
`#[contract]`/`#[contracttype]`/`#[contractclient]` expand to undocumented
public items we cannot annotate, so `missing_docs` warnings whose span is
one of those attribute lines are ignored. Everything else (undocumented
entrypoints, fields, variants, broken intra-doc links) fails the build.

The `missing_docs` lint is enabled crate-wide for rustdoc only
(`#![cfg_attr(doc, warn(missing_docs))]`), so ordinary builds and clippy
are unaffected.

Usage: scripts/doc-check.py [package ...]   (default: all documented crates)
"""
import json, subprocess, sys

PACKAGES = ["zenith-common", "zenith-governor", "zenith-grants-escrow",
            "zenith-multisig", "zenith-params", "zenith-staking",
            "zenith-streams", "zenith-vault"]
GENERATED = ("#[contract]", "#[contracttype]", "#[contractclient")

pkgs = sys.argv[1:] or PACKAGES
cmd = ["cargo", "doc", "--no-deps", "--lib", "--keep-going",
       "--message-format=json"] + [a for p in pkgs for a in ("-p", p)]
out = subprocess.run(cmd, capture_output=True, text=True).stdout
bad = 0
for line in out.splitlines():
    try:
        m = json.loads(line)
    except ValueError:
        continue
    msg = m.get("message")
    if m.get("reason") != "compiler-message" or not msg:
        continue
    if msg["level"] not in ("warning", "error") or not msg["spans"]:
        continue
    span = next((s for s in msg["spans"] if s["is_primary"]), None)
    if (msg.get("code") or {}).get("code") == "missing_docs" and span:
        text = span["text"][0]["text"].strip() if span["text"] else ""
        if text.startswith(GENERATED):
            continue
    where = f'{span["file_name"]}:{span["line_start"]}' if span else "?"
    print(f'{where}: {msg["message"]}')
    bad += 1
if bad:
    sys.exit(f"doc-check: {bad} problem(s)")
print("doc-check: ok")
