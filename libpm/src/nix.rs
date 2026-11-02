use std::{
    fs,
    process::{Command, Stdio},
};

use anyhow::{bail, ensure, Context, Result};

use crate::InstallOutcome;

const NIXPKGS: &str = "nixpkgs";
const EXPERIMENTAL_FEATURES: &str = "nix-command flakes";
const GPU: &str = include_str!("../../gpu.nix");
// Wheel can write to this profile (see pond-nix-gpu.conf), so updating it
// needs no pkexec prompt beyond Paru's.
const GPU_PROFILE: &str = "/nix/var/nix/profiles/pond/gpu";

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
    let mut command = Command::new("nix");
    command
        .arg("--extra-experimental-features")
        .arg(EXPERIMENTAL_FEATURES)
        .args(["build", "--impure", "--profile", GPU_PROFILE, "--expr", GPU])
        .stdin(Stdio::null());
    // Match the host's NVIDIA libraries, which Paru may have just updated.
    let nvidia = fs::read_dir("/usr/lib")
        .into_iter()
        .flatten()
        .flatten()
        .find_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.strip_prefix("libnvidia-glcore.so.").map(str::to_owned)
        });
    if let Some(version) = nvidia {
        command.args(["--argstr", "version", &version]);
    }
    let status = command.status().context("starting Nix")?;
    ensure!(
        status.success(),
        "building the Nix GPU profile failed ({status})"
    );
    Ok(())
}
