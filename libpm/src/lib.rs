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

pub const ICON_DIR: &str = match option_env!("POND_PM_ICON_DIR") {
    Some(dir) => dir,
    None => "/usr/share/pond-pm/icons",
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct App {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub package_source: PackageSource,
    pub traffic: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl App {
    pub fn icon_path(&self) -> Option<String> {
        self.icon.as_ref().map(|hash| format!("{ICON_DIR}/{hash}.webp"))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PackageSource {
    pub manager: String,
    pub package: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallOutcome {
    Installed,
    AlreadyInstalled,
}

pub struct Catalog {
    search: search::Search,
}

impl Catalog {
    pub fn load() -> Result<Self> {
        let apps: Vec<App> = serde_json::from_slice(CATALOG).context("parsing catalog.json")?;
        let search = search::Search::build(apps)?;

        Ok(Self { search })
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<App>> {
        self.search.query(query, limit)
    }

    pub fn install(&self, app_id: u64) -> Result<InstallOutcome> {
        let app = self
            .search
            .apps()
            .iter()
            .find(|app| app.id == app_id)
            .with_context(|| format!("app {app_id} is not in the package catalog"))?;

        let status = install_status::InstallStatus::start(app, "installing", "installed");
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

pub struct Installed {
    search: search::Search,
}

impl Installed {
    pub fn load() -> Result<Self> {
        let catalog: Vec<App> = serde_json::from_slice(CATALOG).context("parsing catalog.json")?;
        let apps: Vec<App> = Self::packages()?
            .into_iter()
            .enumerate()
            .map(|(id, source)| {
                let known = catalog.iter().find(|app| {
                    let listed = &app.package_source;
                    listed.package == source.package
                        && listed.manager.replace("aur", "pacman") == source.manager
                });
                App {
                    id: id as u64,
                    name: known.map_or_else(|| source.package.clone(), |app| app.name.clone()),
                    description: source.package.clone(),
                    traffic: known.map_or(0, |app| app.traffic + 1),
                    icon: known.and_then(|app| app.icon.clone()),
                    package_source: source,
                }
            })
            .collect();
        let search = search::Search::build(apps)?;

        Ok(Self { search })
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<App>> {
        self.search.query(query, limit)
    }

    pub fn uninstall(&self, source: &PackageSource) -> Result<()> {
        let app = self
            .search
            .apps()
            .iter()
            .find(|app| app.package_source == *source)
            .with_context(|| format!("{} is not installed", source.package))?;

        let status = install_status::InstallStatus::start(app, "uninstalling", "uninstalled");
        let result = match source.manager.as_str() {
            "flatpak" => flatpak::uninstall(&source.package),
            "nix" => nix::uninstall(&source.package),
            "pacman" | "aur" => paru::uninstall(&source.package),
            manager => Err(anyhow!(
                "uninstalling with package manager {manager:?} is not supported"
            )),
        };
        status.finish(&result);
        result
    }

    fn packages() -> Result<Vec<PackageSource>> {
        let mut packages = Vec::new();
        for (manager, installed) in [
            ("pacman", paru::installed()?),
            ("flatpak", flatpak::installed()?),
            ("nix", nix::installed()?),
        ] {
            packages.extend(installed.into_iter().map(|package| PackageSource {
                manager: manager.to_owned(),
                package,
            }));
        }
        Ok(packages)
    }
}
