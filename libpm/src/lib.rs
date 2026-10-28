//! Package management library.

mod flatpak;
mod install_status;
mod nix;
mod paru;
mod search;
mod update;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

pub use update::{update, updates_available};

const CATALOG: &[u8] = include_bytes!("../../catalog.json");

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct App {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub package_source: PackageSource,
    pub traffic: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PackageSource {
    pub manager: String,
    pub package: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallOutcome {
    Installed,
    AlreadyInstalled,
}

pub struct PackageManager {
    apps: Vec<App>,
    search: search::Search,
}

impl PackageManager {
    pub fn init() -> Result<Self> {
        let apps: Vec<App> = serde_json::from_slice(CATALOG).context("parsing catalog.json")?;
        let search = search::Search::build(&apps)?;

        Ok(Self { apps, search })
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<App>> {
        self.search.query(query, limit)
    }

    pub fn install(&self, app_id: u64) -> Result<InstallOutcome> {
        let app = self
            .apps
            .iter()
            .find(|app| app.id == app_id)
            .with_context(|| format!("app {app_id} is not in the package catalog"))?;

        let status = install_status::InstallStatus::start(app);
        let result = match app.package_source.manager.as_str() {
            "flatpak" => flatpak::install(&app.package_source.package, status.progress()),
            "nix" => nix::install(&app.package_source.package),
            "pacman" | "aur" => paru::install(&app.package_source.package, status.progress()),
            manager => Err(anyhow!("package manager {manager:?} is not supported")),
        };
        status.finish(&result);
        result
    }
}
