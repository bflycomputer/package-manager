use std::io::{self, BufRead, Write};

use anyhow::{bail, Context, Result};
use libpm::PackageManager;
use serde_json::json;

fn search() -> Result<()> {
    let manager = PackageManager::init()?;
    let mut output = io::stdout().lock();

    for line in io::stdin().lock().lines() {
        let query: String = serde_json::from_str(&line?)?;
        let response = match manager.search(&query, 50) {
            Ok(apps) => json!({ "query": query, "apps": apps }),
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
        Some("search") => search()?,
        Some("install") => {
            let id = args.next().context("missing app ID")?.parse()?;
            PackageManager::init()?.install(id)?;
        }
        _ => bail!("usage: pm-extension-bridge search | install <app-id>"),
    }

    Ok(())
}
