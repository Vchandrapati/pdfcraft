//! `cargo xtask clif …`: debug builds and tests with the Cranelift code generator.
//!
//! Cranelift compiles much faster than LLVM because it doesn't optimise, which is what an
//! edit-build-test loop wants. Dependencies are built with it too: on Windows, code from the two
//! backends doesn't agree on how small float structs (egui's `Vec2` and `Rect`, hayro's geometry)
//! are passed, so mixing them gave garbage sizes and access violations in hundreds of tests.
//! The exceptions are signing's crypto: `pdfcraft-sign` and the aws-lc crates under it, because
//! Cranelift emits calls into the statically linked aws-lc C library as DLL imports that don't
//! link, and the ed25519 crates, whose AVX2 backend Cranelift can't compile. Their interfaces are
//! slices and byte buffers, so the boundary is safe.
//!
//! Cranelift can't compile the AVX2 intrinsics that `fearless_simd` (under egui's and hayro's
//! rasterisers) and `pic-scale` (hayro's image scaling) choose at runtime. For these builds only,
//! they are replaced through `[patch]` by copies in `target/clif/` whose CPU-feature checks say
//! "no", so every SIMD user takes the portable path; Cargo.lock stays untouched, as the patched
//! graph is locked in `target/clif/Cargo.lock`.
//!
//! Panics abort instead of unwinding under Cranelift on Windows, so a failing test aborts its
//! process (nextest still reports it). `clif test` uses nextest's `clif` profile
//! (`.config/nextest.toml`), which leaves out the tests Cranelift can't run as written: those
//! that catch a panic on purpose, the timing budgets unoptimised code misses, and the nesting
//! limits its larger stack frames reach first. `cargo xtask ci` still runs them all.
//!
//! The backend is a nightly-only rustup component, so this runs `cargo` on the nightly toolchain
//! (`rustup toolchain install nightly --component rustc-codegen-cranelift-preview`) with the
//! profile set on the command line; ordinary `cargo` and `cargo xtask ci` are unaffected and
//! remain the gate before committing. Output goes to `target/clif/` so the two toolchains don't
//! rebuild each other's artifacts.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::gates::root;

const USAGE: &str = "usage: cargo xtask clif <cargo command> [args]   e.g. clif build -p pdfcraft, clif run -p pdfcraft -- file.pdf,
       clif test [nextest args]   (nextest run --workspace when no args, plus the doc tests)";

/// Profile settings: Cranelift for everything except signing (see the module docs).
const PROFILE: [&str; 7] = [
    r#"profile.dev.codegen-backend="cranelift""#,
    r#"profile.dev.package."*".codegen-backend="cranelift""#,
    r#"profile.dev.package.pdfcraft-sign.codegen-backend="llvm""#,
    r#"profile.dev.package.aws-lc-rs.codegen-backend="llvm""#,
    r#"profile.dev.package.aws-lc-sys.codegen-backend="llvm""#,
    r#"profile.dev.package.ed25519-dalek.codegen-backend="llvm""#,
    r#"profile.dev.package.curve25519-dalek.codegen-backend="llvm""#,
];

/// Dependencies that pick AVX2 code at runtime, patched to never detect it.
const NO_SIMD_DETECTION: [&str; 2] = ["fearless_simd", "pic-scale"];

fn clif_dir() -> PathBuf {
    root().join("target").join("clif")
}

/// `--config` values: the profile, the lockfile and the patched dependencies.
fn settings() -> anyhow::Result<Vec<String>> {
    let dir = clif_dir();
    std::fs::create_dir_all(&dir)?;
    // The patched graph's lockfile, refreshed from the real one so versions follow it.
    let lock = dir.join("Cargo.lock");
    std::fs::copy(root().join("Cargo.lock"), &lock).context("copying Cargo.lock")?;
    let mut out: Vec<String> = PROFILE.iter().map(|s| s.to_string()).collect();
    out.push(format!("resolver.lockfile-path={}", toml_path(&lock)));
    for (name, src) in registry_sources(&NO_SIMD_DETECTION)? {
        let patched = without_simd_detection(&name, &src)?;
        out.push(format!("patch.crates-io.{name}.path={}", toml_path(&patched)));
    }
    Ok(out)
}

fn toml_path(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\\', "/"))
}

/// The registry source folder of each named dependency.
fn registry_sources(names: &[&str]) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let out =
        Command::new("cargo").current_dir(root()).args(["metadata", "--format-version", "1", "--offline"]).output().context("cargo metadata")?;
    if !out.status.success() {
        bail!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr));
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let mut found = Vec::new();
    for p in meta["packages"].as_array().into_iter().flatten() {
        let name = p["name"].as_str().unwrap_or_default();
        if names.contains(&name) && p["source"].as_str().is_some_and(|s| s.starts_with("registry+")) {
            let manifest = PathBuf::from(p["manifest_path"].as_str().unwrap_or_default());
            let dir = manifest.parent().context("a manifest has a folder")?.to_path_buf();
            found.push((name.to_string(), dir));
        }
    }
    Ok(found)
}

/// A copy of the crate at `src` (made once, so it isn't rebuilt every run) in which every
/// `is_x86_feature_detected!(…)` is `false`.
fn without_simd_detection(name: &str, src: &Path) -> anyhow::Result<PathBuf> {
    let folder = src.file_name().context("a crate folder has a name")?;
    let dest = clif_dir().join("no-simd-detection").join(folder);
    let done = dest.join(".pdfcraft-patched");
    if done.exists() {
        return Ok(dest);
    }
    let _ = std::fs::remove_dir_all(&dest);
    let mut replaced = 0;
    copy_tree(src, &dest, &mut |text| {
        let (patched, n) = detection_off(text);
        replaced += n;
        patched
    })?;
    if replaced == 0 {
        bail!("{name}: found no CPU-feature checks to patch; update NO_SIMD_DETECTION in xtask/src/clif.rs");
    }
    std::fs::write(&done, format!("{replaced} checks replaced\n"))?;
    Ok(dest)
}

fn copy_tree(src: &Path, dest: &Path, rewrite: &mut dyn FnMut(&str) -> String) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &to, rewrite)?;
        } else if entry.path().extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(entry.path())?;
            std::fs::write(&to, rewrite(&text))?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// `text` with every `[std::arch::]is_x86_feature_detected!(…)` replaced by `false`, and how many.
fn detection_off(text: &str) -> (String, usize) {
    const MACRO: &str = "is_x86_feature_detected!(";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut n = 0;
    while let Some(at) = rest.find(MACRO) {
        let mut head = &rest[..at];
        for prefix in ["std::arch::", "core::arch::"] {
            head = head.strip_suffix(prefix).unwrap_or(head);
        }
        // The macro's arguments are string literals, so the first `)` closes it.
        let Some(close) = rest[at..].find(')') else { break };
        out.push_str(head);
        out.push_str("false");
        rest = &rest[at + close + 1..];
        n += 1;
    }
    out.push_str(rest);
    (out, n)
}

/// `cargo <command> -Zcodegen-backend --config … <rest>` on nightly. The settings follow the
/// subcommand because cargo doesn't pass its own global options on to an external one: nextest
/// reads them itself, and cargo's built-in commands accept them there too.
fn nightly_cargo(command: &[&str], settings: &[String], rest: &[&str]) -> Command {
    let mut c = Command::new("rustup");
    c.current_dir(root()).args(["run", "nightly", "cargo"]).args(command).arg("-Zcodegen-backend");
    for setting in settings {
        c.args(["--config", setting]);
    }
    c.args(rest);
    // `cargo xtask` runs under the stable toolchain; don't let its tools leak into the nightly run.
    for var in ["CARGO", "RUSTC", "RUSTDOC", "RUSTUP_TOOLCHAIN"] {
        c.env_remove(var);
    }
    c.env("CARGO_TARGET_DIR", clif_dir());
    c.env("NEXTEST_PROFILE", "clif");
    c
}

fn run(command: &[&str], settings: &[String], rest: &[&str]) -> anyhow::Result<()> {
    let what = format!("cargo +nightly (cranelift) {} {}", command.join(" "), rest.join(" "));
    eprintln!("$ {what}");
    let status = nightly_cargo(command, settings, rest).status().with_context(|| format!("{what}: failed to spawn rustup"))?;
    if !status.success() {
        bail!("{what}: exited with {status}");
    }
    Ok(())
}

fn ready() -> anyhow::Result<()> {
    let out = Command::new("rustup")
        .args(["component", "list", "--installed", "--toolchain", "nightly"])
        .output()
        .context("rustup is needed for the nightly toolchain")?;
    if !String::from_utf8_lossy(&out.stdout).contains("rustc-codegen-cranelift") {
        bail!("the Cranelift backend is not installed; run `rustup toolchain install nightly --component rustc-codegen-cranelift-preview`");
    }
    Ok(())
}

pub fn clif(args: &[String]) -> anyhow::Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let Some((&command, rest)) = args.split_first() else { bail!("{USAGE}") };
    ready()?;
    let settings = settings()?;
    if command != "test" {
        return run(&[command], &settings, rest);
    }
    let rest = if rest.is_empty() { &["--workspace"][..] } else { rest };
    // Tests as in `cargo xtask ci`: nextest when installed, then the doc tests nextest skips.
    let nextest = Command::new("cargo").args(["nextest", "--version"]).output().is_ok_and(|o| o.status.success());
    if !nextest {
        return run(&["test"], &settings, rest);
    }
    run(&["nextest", "run"], &settings, rest)?;
    if rest == ["--workspace"] {
        run(&["test"], &settings, &["--workspace", "--doc"])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::detection_off;

    #[test]
    fn feature_checks_become_false_with_or_without_a_path() {
        let (out, n) = detection_off(
            r#"if std::arch::is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") { a() } else if core::arch::is_x86_feature_detected!("sse4.1") {}"#,
        );
        assert_eq!(n, 3);
        assert_eq!(out, "if false && false { a() } else if false {}");
        assert_eq!(detection_off("no checks here"), ("no checks here".to_string(), 0));
    }
}
