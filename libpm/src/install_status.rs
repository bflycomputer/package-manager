//! Best-effort install updates through Pond's existing Quickshell IPC.
use std::{cell::Cell, process::Command, process::Stdio, rc::Rc};

use anyhow::Result;

use crate::{App, InstallOutcome};

#[derive(Clone)]
pub(crate) struct InstallStatus {
    name: String,
    percent: Rc<Cell<i32>>,
}

impl InstallStatus {
    pub(crate) fn start(app: &App) -> Self {
        let status = Self {
            name: app.name.clone(),
            percent: Rc::new(Cell::new(-1)),
        };
        status.send("installing", "");
        status
    }

    pub(crate) fn progress(&self) -> impl Fn(f64) + 'static {
        let status = self.clone();
        move |value| {
            if value.is_finite() {
                let percent = (value.clamp(0.0, 0.99) * 100.0).round() as i32;
                if percent > status.percent.get() {
                    status.percent.set(percent);
                    status.send("installing", "");
                }
            }
        }
    }

    pub(crate) fn finish(self, result: &Result<InstallOutcome>) {
        match result {
            Ok(_) => {
                self.percent.set(100);
                self.send("installed", "");
            }
            Err(error) => self.send("failed", &format!("{error:#}")),
        }
    }

    fn send(&self, status: &str, message: &str) {
        let _ = Command::new("qs")
            .args(["-c", "pond-shell", "ipc", "call", "installs", "update"])
            .args([
                &std::process::id().to_string(),
                &self.name,
                status,
                &self.percent.get().to_string(),
                message,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
