use std::io::{self, BufRead, Write};

use anyhow::{bail, Context, Result};
use libpm::{App, Catalog, Installed, PackageSource};
use serde_json::json;

fn serve(search: impl Fn(&str) -> Result<Vec<App>>) -> Result<()> {
    let mut output = io::stdout().lock();

    for line in io::stdin().lock().lines() {
        let query: String = serde_json::from_str(&line?)?;
        let response = match search(&query) {
            Ok(apps) => json!({
                "query": query,
                "apps": apps
                    .iter()
                    .map(|app| {
                        json!({
                            "id": app.id,
                            "name": app.name,
                            "icon": app.icon_path(),
                            "source": app.package_source,
                        })
                    })
                    .collect::<Vec<_>>(),
            }),
            Err(error) => {
                eprintln!("{error:#}");
                json!({ "query": query, "error": true })
            }
        };
        serde_json::to_writer(&mut output, &response)?;
        writeln!(output)?;
        output.flush()?;
    }

    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);

    match args.next().as_deref() {
        Some("catalog") => {
            let catalog = Catalog::load()?;
            serve(|query| catalog.search(query, 50))?;
        }
        Some("installed") => {
            let installed = Installed::load()?;
            serve(|query| installed.search(query, 50))?;
        }
        Some("install") => {
            let id = args.next().context("missing app ID")?.parse()?;
            Catalog::load()?.install(id)?;
        }
        Some("uninstall") => {
            let manager = args.next().context("missing package manager")?;
            let package = args.next().context("missing package name")?;
            Installed::load()?.uninstall(&PackageSource { manager, package })?;
        }
        _ => bail!(
            "usage: pm-extension-bridge catalog | installed | install <app-id> | uninstall <manager> <package>"
        ),
    }

    Ok(())
}
