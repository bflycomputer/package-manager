use std::{
    fs,
    process::{Command, Stdio},
};

use anyhow::{bail, ensure, Context, Result};

use crate::InstallOutcome;

const NIXPKGS: &str = "nixpkgs";
const EXPERIMENTAL_FEATURES: &str = "nix-command flakes";
const GPU: &str = include_str!("../../gpu.nix");
const GPU_LINK: &str = "/var/lib/pond/gpu";

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

pub(crate) fn update_gpu() -> Result<()> {
    let mut build = Command::new("pkexec");
    build
        .arg("nix")
        .arg("--extra-experimental-features")
        .arg(EXPERIMENTAL_FEATURES)
        .args(["build", "--impure", "--expr", GPU, "--out-link", GPU_LINK])
        .stdin(Stdio::null());
    if let Ok(library) = fs::read_link("/usr/lib/libnvidia-ml.so.1") {
        let version = library
            .to_str()
            .and_then(|name| name.strip_prefix("libnvidia-ml.so."))
            .context("reading the NVIDIA driver version")?;
        build.args(["--argstr", "version", version]);
    }
    let status = build
        .status()
        .context("starting Nix to build GPU drivers")?;
    ensure!(
        status.success(),
        "building Nix GPU drivers failed ({status})"
    );
    Ok(())
}
