use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::InstallOutcome;

const NIXPKGS: &str = "nixpkgs";
const EXPERIMENTAL_FEATURES: &str = "nix-command flakes";

pub(crate) fn install(package: &str) -> Result<InstallOutcome> {
    let installable = format!("{NIXPKGS}#{package}");
    let output = Command::new("nix")
        .arg("--extra-experimental-features")
        .arg(EXPERIMENTAL_FEATURES)
        .args(["profile", "add", "--impure"])
        .arg(&installable)
        .env("NIXPKGS_ALLOW_UNFREE", "1")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("starting Nix to install {installable}"))?;

    if output.status.success() {
        return Ok(InstallOutcome::Installed);
    }

    let diagnostic = String::from_utf8_lossy(&output.stderr);
    let diagnostic = diagnostic.trim();

    bail!(
        "installing {installable} with Nix failed ({}): {diagnostic}",
        output.status
    );
}
