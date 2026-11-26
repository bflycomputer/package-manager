//! Best-effort install updates through Pond's existing Quickshell IPC.
use std::{cell::Cell, process::Command, process::Stdio, rc::Rc};

use anyhow::Result;

use crate::App;

#[derive(Clone)]
pub(crate) struct InstallStatus {
    name: String,
    icon: String,
    percent: Rc<Cell<i32>>,
    running: &'static str,
    done: &'static str,
}

impl InstallStatus {
    pub(crate) fn start(app: &App, running: &'static str, done: &'static str) -> Self {
        let status = Self {
            name: app.name.clone(),
            icon: app.icon_path().unwrap_or_default(),
            percent: Rc::new(Cell::new(-1)),
            running,
            done,
        };
        status.send(running, "");
        status
    }

    pub(crate) fn progress(&self) -> impl Fn(f64) + 'static {
        let status = self.clone();
        move |value| {
            if value.is_finite() {
                let percent = (value.clamp(0.0, 0.99) * 100.0).round() as i32;
                if percent > status.percent.get() {
                    status.percent.set(percent);
                    status.send(status.running, "");
                }
            }
        }
    }

    pub(crate) fn finish<T>(self, result: &Result<T>) {
        match result {
            Ok(_) => {
                self.percent.set(100);
                self.send(self.done, "");
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
                &self.icon,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
