use std::{
    process::{Command, Stdio},
    rc::Rc,
};

use anyhow::{bail, ensure, Context, Result};
use libflatpak::prelude::*;

use crate::InstallOutcome;

const REMOTE: &str = "flathub";
const BRANCH: &str = "stable";

pub(crate) fn update() -> Result<()> {
    let status = Command::new("flatpak")
        .args(["update", "--assumeyes"])
        .stdin(Stdio::null())
        .status()
        .context("starting Flatpak")?;
    ensure!(status.success(), "updating Flatpaks failed ({status})");
    Ok(())
}

pub(crate) fn install(package: &str, report: impl Fn(f64) + 'static) -> Result<InstallOutcome> {
    let installation = libflatpak::Installation::new_system(None::<&libflatpak::gio::Cancellable>)
        .context("opening the system Flatpak installation")?;
    let architecture = libflatpak::default_arch().context("determining Flatpak architecture")?;
    let reference = format!("app/{package}/{architecture}/{BRANCH}");
    let transaction = libflatpak::Transaction::for_installation(
        &installation,
        None::<&libflatpak::gio::Cancellable>,
    )
    .context("creating a Flatpak transaction")?;

    match transaction.add_install(REMOTE, &reference, &[]) {
        Ok(()) => {}
        Err(error) if error.matches(libflatpak::Error::AlreadyInstalled) => {
            return Ok(InstallOutcome::AlreadyInstalled);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("adding {reference} from {REMOTE} to the transaction"));
        }
    }

    let report = Rc::new(report);
    transaction.connect_new_operation(move |transaction, operation, progress| {
        // Flatpak reports each app/runtime separately, in execution order.
        let operations = transaction.operations();
        let completed = operations.iter().position(|op| op == operation).unwrap();
        let total = operations.len();
        let report = report.clone();
        progress.set_update_frequency(100);
        let update = move |progress: &libflatpak::TransactionProgress| {
            report((completed as f64 + progress.progress() as f64 / 100.0) / total as f64);
        };
        update(progress);
        progress.connect_changed(update);
    });

    match transaction.run(None::<&libflatpak::gio::Cancellable>) {
        Ok(()) => Ok(InstallOutcome::Installed),
        Err(error) if error.matches(libflatpak::Error::AlreadyInstalled) => {
            Ok(InstallOutcome::AlreadyInstalled)
        }
        Err(error) => Err(error).with_context(|| format!("installing {reference} from {REMOTE}")),
    }
}

fn installations() -> Result<[libflatpak::Installation; 2]> {
    let cancellable = None::<&libflatpak::gio::Cancellable>;
    Ok([
        libflatpak::Installation::new_system(cancellable)
            .context("opening the system Flatpak installation")?,
        libflatpak::Installation::new_user(cancellable)
            .context("opening the user Flatpak installation")?,
    ])
}

pub(crate) fn installed() -> Result<Vec<String>> {
    let mut apps = Vec::new();

    for installation in installations()? {
        let installed = installation
            .list_installed_refs_by_kind(
                libflatpak::RefKind::App,
                None::<&libflatpak::gio::Cancellable>,
            )
            .context("listing installed Flatpak apps")?;
        apps.extend(installed.iter().filter_map(|app| app.name()).map(String::from));
    }

    apps.sort();
    apps.dedup();
    Ok(apps)
}

pub(crate) fn uninstall(package: &str) -> Result<()> {
    let cancellable = None::<&libflatpak::gio::Cancellable>;

    for installation in installations()? {
        let reference = match installation.current_installed_app(package, cancellable) {
            Ok(installed) => installed.format_ref().context("formatting the Flatpak ref")?,
            Err(error) if error.matches(libflatpak::Error::NotInstalled) => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("looking up installed {package}"));
            }
        };
        let transaction = libflatpak::Transaction::for_installation(&installation, cancellable)
            .context("creating a Flatpak transaction")?;
        transaction
            .add_uninstall(&reference)
            .with_context(|| format!("adding {reference} to the transaction"))?;
        return transaction
            .run(cancellable)
            .with_context(|| format!("uninstalling {reference}"));
    }

    bail!("{package} is not an installed Flatpak app");
}
