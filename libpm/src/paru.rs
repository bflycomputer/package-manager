use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
    process::{Command, ExitStatus, Stdio},
};

use anyhow::{bail, Context, Result};

use crate::InstallOutcome;

pub(crate) fn install(package: &str, report: impl Fn(f64)) -> Result<InstallOutcome> {
    let _lock = lock(Path::new("/var/lib/pacman"))?;
    let mut command = Command::new("paru");
    command
        .args([
            "--sudo",
            "pkexec",
            "--nosudoloop",
            "--skipreview",
            "--pgpfetch",
            "--useask",
            "-S",
            "--needed",
            "--noconfirm",
            "--ask",
            "4",
            "--color",
            "never",
            "--",
        ])
        .arg(package)
        .env("LC_ALL", "C");
    let status = run_with_progress(command, report)
        .with_context(|| format!("running Paru to install {package}"))?;

    if status.success() {
        return Ok(InstallOutcome::Installed);
    }

    bail!("installing {package} with Paru failed ({status})");
}

pub(crate) fn lock(database: &Path) -> Result<File> {
    let lock = File::open(database).context("opening the Pacman database directory")?;
    lock.lock().context("waiting for another Pacman request")?;
    Ok(lock)
}

pub(crate) fn update() -> Result<()> {
    let _lock = lock(Path::new("/var/lib/pacman"))?;
    let status = Command::new("paru")
        .args([
            "--sudo",
            "pkexec",
            "--nosudoloop",
            "-Syu",
            "--skipreview",
            "--noupgrademenu",
            "--nonewsonupgrade",
            "--noconfirm",
            "--useask",
            "--ask",
            "4",
            "--pgpfetch",
            "--failfast",
        ])
        .stdin(Stdio::null())
        .status()
        .context("starting Paru")?;

    if status.success() {
        return Ok(());
    }

    bail!("updating packages with Paru failed ({status})");
}

fn run_with_progress(command: Command, report: impl Fn(f64)) -> Result<ExitStatus> {
    // script provides terminal output for Pacman's percentages. Quote each
    // argument for a fixed shell and keep the actual installer's input closed.
    let invocation = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|arg| format!("'{}'", arg.to_string_lossy().replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ");
    let mut child = Command::new("script")
        .args([
            "-qefc",
            &format!("exec {invocation} </dev/null"),
            "/dev/null",
        ])
        .envs(
            command
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        )
        .env("SHELL", "/bin/sh")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut output = child.stdout.take().unwrap();
    let mut parser = DownloadProgress::default();
    let mut buffer = [0; 4096];
    let read_result = loop {
        match output.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => break Err(error),
            Ok(count) => {
                let _ = io::stdout().write_all(&buffer[..count]);
                let _ = io::stdout().flush();
                parser.feed(&buffer[..count], &report);
            }
        }
    };
    let status = child.wait()?;
    read_result.context("reading Paru output")?;
    Ok(status)
}

#[derive(Default)]
struct DownloadProgress {
    line: Vec<u8>,
    total: bool,
    curl: bool,
}

impl DownloadProgress {
    fn feed(&mut self, bytes: &[u8], report: &impl Fn(f64)) {
        for &byte in bytes {
            if byte == b'\r' || byte == b'\n' {
                if let Some(value) = self.parse_line() {
                    report(value * 0.9);
                }
                self.line.clear();
            } else if self.line.len() < 4096 {
                self.line.push(byte);
            }
        }
    }

    fn parse_line(&mut self) -> Option<f64> {
        let line = String::from_utf8_lossy(&self.line);
        let line = line.trim();
        if line.contains(":: Retrieving packages...") {
            self.total = false;
        }
        if line.contains("% Total") && line.contains("% Received") {
            self.curl = true;
        }
        if line.contains("/s ") && line.contains('[') {
            let is_total = line.contains("Total (");
            self.total |= is_total;
            if self.total && !is_total {
                return None;
            }
            let before = line.rsplit_once('%')?.0;
            let percent: f64 = before.split_whitespace().last()?.parse().ok()?;
            return (0.0..=100.0).contains(&percent).then_some(percent / 100.0);
        }
        // makepkg's default curl downloader prints a twelve-column progress table.
        if self.curl {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() == 12 && fields[8].contains(':') {
                let percent: f64 = fields[0].parse().ok()?;
                if percent == 100.0 {
                    self.curl = false;
                }
                return (0.0..=100.0).contains(&percent).then_some(percent / 100.0);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn downloads_use_total_and_ignore_installation_percentages() {
        let output = b":: Retrieving packages...\n\x1b[2K Total (0/2) 1 MiB 1 MiB/s 00:01 [##--]  25%\r\x1b[2A first 1 MiB 1 MiB/s 00:00 [####] 100%\r\x1b[1B Total (1/2) 2 MiB 1 MiB/s 00:01 [###-]  75%\r(1/2) installing example [####] 100%\n Total (2/2) 3 MiB 1 MiB/s 00:00 [####] 100%\r";
        for chunk_size in [1, 2, 17, 4096] {
            let mut parser = DownloadProgress::default();
            let values = RefCell::new(Vec::new());
            for chunk in output.chunks(chunk_size) {
                parser.feed(chunk, &|value| values.borrow_mut().push(value));
            }
            assert_eq!(*values.borrow(), vec![0.25 * 0.9, 0.75 * 0.9, 0.9]);
        }
    }

    #[test]
    fn single_download_and_makepkg_curl_progress() {
        let mut parser = DownloadProgress::default();
        let values = RefCell::new(Vec::new());
        parser.feed(b" app 1 MiB 1 MiB/s 00:01 [##--]  50%\r\n  % Total    % Received % Xferd  Average Speed   Time    Time     Time  Current\n                                 Dload  Upload   Total   Spent    Left  Speed\n\r 25 1024 25 256 0 0 128 0 0:00:08 0:00:02 0:00:06 128\r100 1024 100 1024 0 0 128 0 0:00:08 0:00:08 --:--:-- 128\n", &|value| values.borrow_mut().push(value));
        assert_eq!(*values.borrow(), vec![0.45, 0.225, 0.9]);
    }

    #[test]
    fn child_gets_terminal_output_closed_input_and_preserves_exit_status() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "test \"$1\" = \"$EXPECTED\" || exit 98; test -t 1 && test -t 2 && ! test -t 0 && ! read unused || exit 99; printf ' app 1 MiB 1 MiB/s 00:01 [##--]  50%%\\r' >&2; exit 42", "probe", "quote' and space $(false);false"]);
        command.env("EXPECTED", "quote' and space $(false);false");
        let values = RefCell::new(Vec::new());
        let status = run_with_progress(command, |value| values.borrow_mut().push(value)).unwrap();
        assert_eq!(status.code(), Some(42));
        assert_eq!(*values.borrow(), vec![0.45]);
    }
}
