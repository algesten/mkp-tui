//! Resolves the marketing version baked into `mkp --version`.
//!
//! Resolution order, first hit wins:
//!
//! 1. `MKP_VERSION` — set by `make release` and by every packaging
//!    recipe (AUR `pkgver`, the Homebrew formula, the nix flake),
//!    all of which already know the version they are building.
//! 2. `git describe --tags --abbrev=0` — a development clone with
//!    the release tag propagated into it.
//! 3. `"dev"` — a bare source tarball with no packaging around it.
//!
//! Deliberately NOT `CARGO_PKG_VERSION`: the crate versions in this
//! workspace are not the marketing version and are not bumped per
//! release. See the release flow in the closed repo's Makefile.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MKP_VERSION");

    let version = std::env::var("MKP_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(git_describe)
        .unwrap_or_else(|| "dev".to_string());

    println!("cargo:rustc-env=MKP_VERSION={version}");
}

fn git_describe() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}
