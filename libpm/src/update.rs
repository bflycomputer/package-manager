use std::{fs, path::Path, process::Command, thread};

use anyhow::{anyhow, bail, Context, Result};

pub fn updates_available() -> Result<bool> {
    let output = Command::new("checkupdates")
        .output()
        .context("running checkupdates")?;
    match output.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&output.stdout).lines().count() > 3),
        Some(2) => Ok(false),
        _ => bail!(
            "checkupdates failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

pub fn update() -> Result<bool> {
    let (paru, flatpak, nix) = thread::scope(|scope| {
        let paru = scope.spawn(|| crate::paru::update().and_then(|()| crate::nix::update_gpu()));
        let flatpak = crate::flatpak::update();
        let nix = crate::nix::update();
        let paru = paru
            .join()
            .unwrap_or_else(|_| Err(anyhow!("Paru update worker panicked")));
        (paru, flatpak, nix)
    });

    let failures: Vec<String> = [("Paru", paru), ("Flatpak", flatpak), ("Nix", nix)]
        .into_iter()
        .filter_map(|(source, result)| Some(format!("{source} update failed: {:#}", result.err()?)))
        .collect();
    if !failures.is_empty() {
        bail!("{}", failures.join("\n"));
    }
    Ok(restart_recommended().unwrap_or(false))
}

fn restart_recommended() -> Result<bool> {
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")?;
    if !Path::new("/usr/lib/modules")
        .join(kernel.trim())
        .join("vmlinuz")
        .exists()
    {
        return Ok(true);
    }
    if let Ok(nvidia) = fs::read_to_string("/sys/module/nvidia/version") {
        if !Path::new(&format!("/usr/lib/libnvidia-glcore.so.{}", nvidia.trim())).exists() {
            return Ok(true);
        }
    }
    Ok(false)
}
