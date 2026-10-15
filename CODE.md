# PdfCraft — instructions for agents

PdfCraft is a clean-room, open-source, Rust-native PDF application. It runs natively on macOS, Windows, Linux and FreeBSD, and on the web via WASM. This repository is a personal, custom fork of upstream PdfCraft (`storytold/pdfcraft`) and is not merged back (`AGENTS.md`).

## Start every session here
1. Read `docs/architecture.md` (crates, layers, the editing model) and the README of the crate you're touching.
2. The user decides what to work on. Upstream's planning files (`plan/`, the milestone/progress logs in `ROADMAP.md` and `docs/roadmap.md`, `docs/gaps.md`, `docs/target-app-parity.md` and `parity/`) are historical reference only: you don't need to read or update them.

## Non-negotiables
- **Assets: read `AGENTS.md` §1 before adding or showing any icon, image, font or document.** No assets from Adobe products, ever. Only openly licensed or contributor-original assets are allowed, each with an entry in `ATTRIBUTION.toml`. `cargo xtask assets` enforces this. `AGENTS.md` overrides this file.
- **Fonts live in [storytold/craft-fonts](https://github.com/storytold/craft-fonts).** Never commit font files here (`AGENTS.md` §1.4). It is the optional build input `CRAFT_FONTS_DIR`: `git clone https://github.com/storytold/craft-fonts ../craft-fonts && CRAFT_FONTS_DIR=../craft-fonts cargo test --workspace` embeds the Japanese fonts (UI fallback, Japanese text in edited PDFs) and runs their tests, which otherwise skip. Code using `pdfcraft_fonts::CRAFT_FONTS` must work when it is empty.
- **Clean-room.**
  - Never read, disassemble or copy anything inside the Acrobat bundle (names and listings only). **Never open `Contents/Resources/JavaScripts/`.**
  - Behaviour comes from public docs, specs (ISO 32000-2, the Arlington model) and black-box observation.
  - Never copy GPL/AGPL code. MuPDF, Ghostscript, Poppler, veraPDF and DSS run only as external oracle processes.
- **Privacy.** When observing Acrobat, use synthetic fixtures only. Never capture the Home view, recent files or account info. Never commit Acrobat outputs, corpus files or personal data.
- **Layering.** Nothing below L7 depends on egui/winit/eframe/rfd (`docs/architecture.md`, enforced by `cargo xtask layers`). `cos`/`filters`/`crypt` stay standalone.
- **Fidelity.** The PDF object graph is the model. Preserve unknown data. Saves are incremental unless a full rewrite is required. Never silently drop or repair data without recording it.
- **Never panic.** Every PDF, script, MCP call, settings file and keystroke is untrusted input, and none of it may crash the app or lose the user's work. This outranks feature work. Return `Result` (the crate's error enum) and propagate with `?`, or fall back leniently and record it. In non-test code:
  - No `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!` unless it is provably infallible, with a comment saying why.
  - No indexing or slicing with input-derived positions (use `get`, and slice strings only at char boundaries).
  - Use checked or saturating arithmetic on input-derived numbers, and guard against division by zero and NaN/inf casts.
  - Cap allocations sized by input, and bound recursion with depth limits or seen-sets.
  - Handle lock poisoning.
  - No `unsafe` (`unsafe_code = "forbid"`).
  - Every crash fix gets a synthetic regression test. See `AGENTS.md` §4.
- **Rust only** in the product and build (`xtask`). No handwritten JS/TS.
- **Quality gates** before every commit: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, the tests (`cargo nextest run --workspace` plus `cargo test --workspace --doc`; plain `cargo test --workspace` without nextest) and `cargo xtask wasm`, all run by `cargo xtask ci`.
- **Commits:** one focused change per commit, with a short descriptive message. Commit only green states. End messages with the attribution line required by the environment.

## Engineering Principles

- **Prefer simplicity:** Implement the smallest clear solution that meets the requirements. Avoid speculative complexity, premature abstractions, and unnecessary dependencies.
- **Proportional testing:** Test meaningful behaviour, edge cases, and regression risks. Do not generate excessive tests for trivial logic, implementation details, or obvious language behaviour.
- **Avoid defensive overengineering:** Do not add redundant validation, checksums, hashing, integrity checks, fallback paths, or recovery mechanisms without a concrete requirement or realistic failure scenario.
- **Trust internal contracts:** Assume validated internal data and established interfaces behave as specified. Validate at external or untrusted boundaries rather than repeatedly checking throughout the codebase.
- **Fail clearly:** Prefer explicit errors over silent fallbacks, broad exception handling, excessive retries, or attempts to recover from unexpected states.
- **Keep abstractions justified:** Introduce abstractions only when they meaningfully reduce duplication, complexity, or coupling. Avoid unnecessary wrappers, interfaces, factories, and helper functions.
- **Minimise changes:** Make focused modifications without unrelated refactoring, formatting, compatibility layers, or additional features.
- **Avoid speculative future-proofing:** Build for current requirements, not hypothetical future use cases. Do not introduce extensibility mechanisms without a demonstrated need.
- **Use existing conventions:** Follow the project's established architecture, patterns, and dependencies rather than introducing new ones without clear benefit.
- **Exercise judgement:** Not every potential edge case needs handling. Prioritise realistic risks and maintainability over theoretical completeness.

## Running and looking at the app
- `cargo run -p pdfcraft -- <file.pdf>` opens the desktop app.
- `cargo run -p pdfcraft-cli -- run --script steps.json --root DIR` drives the engine headlessly through the automation tools (`pdfcraft-cli tools` lists them). Use it, together with `page_render`, to check engine changes. `pdfcraft-cli mcp` is the opt-in MCP server (AGENTS.md §3).
- `cargo xtask fuzz --time 300` mutation-fuzzes open/render/edit/save in child processes. Findings land in `fuzz-out/findings/` (git-ignored; never commit corpus-derived files). Turn every real finding into a small synthetic regression test before fixing it.
- `cargo xtask demo-pdf` builds `dist/demo/pdfcraft-showcase.pdf` (needs Chrome) for visual checks.
- For UI work, **look at the result**. Either launch `pdfcraft --control FILE doc.pdf` and use `pdfcraft-cli ui --control FILE screenshot --out x.png` (plus `inspect`, `click`, `key`, `type`, `command`, `set`), or take a headless shot with `cargo run -p pdfcraft-ui-egui --example shot`. Control-channel tests use kittest (`crates/ui-egui/tests/control.rs`).
- Parallel agents: use a separate `CARGO_TARGET_DIR` per agent and separate git worktrees.

## Current bootstrap debt
- `pdfcraft-render` renders through the vendored `hayro` crate directly and inspects documents through `lopdf` (`vendor/README.md`).
