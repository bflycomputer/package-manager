//! Paru runs `<pm> --pkexec pacman …` instead of `pkexec pacman …`. Every
//! command is forwarded to one long-running broker that starts pkexec itself,
//! so polkit's auth_admin_keep sees the same caller across installs and only
//! asks for a password once every five minutes.

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, IoSlice, IoSliceMut, Read, Write},
    mem::MaybeUninit,
    net::Shutdown,
    os::{
        fd::{AsFd, OwnedFd},
        unix::{
            ffi::OsStringExt,
            net::{UnixListener, UnixStream},
            process::{CommandExt, ExitStatusExt},
        },
    },
    path::PathBuf,
    process::{self, Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use rustix::net::{
    recvmsg, sendmsg,
    sockopt::{set_socket_timeout, Timeout},
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags,
};

pub(crate) const FLAG: &str = "--pkexec";
const BROKER_FLAG: &str = "--pkexec-broker";
// Matches how long polkit keeps an auth_admin_keep authorization.
const IDLE: Duration = Duration::from_secs(5 * 60);

/// Runs the forwarding or broker mode when the arguments ask for it.
pub fn handle_args() {
    let mut args = env::args_os().skip(1);
    let result = match args.next().as_deref().and_then(OsStr::to_str) {
        Some(FLAG) => forward(args.collect()),
        Some(BROKER_FLAG) => broker().map(|()| 0),
        _ => return,
    };
    process::exit(result.unwrap_or_else(|error| {
        eprintln!("{error:#}");
        1
    }));
}

fn socket() -> Result<PathBuf> {
    let runtime = env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?;
    Ok(PathBuf::from(runtime).join("pond-pm-pkexec.sock"))
}

fn forward(args: Vec<OsString>) -> Result<i32> {
    let socket = socket()?;
    let stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(_) => {
            Command::new(env::current_exe()?)
                .arg(BROKER_FLAG)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .context("starting the pkexec broker")?;
            (0..50)
                .find_map(|_| {
                    thread::sleep(Duration::from_millis(100));
                    UnixStream::connect(&socket).ok()
                })
                .context("connecting to the pkexec broker")?
        }
    };

    // The working directory, then each argument, separated by NUL bytes.
    let mut request = env::current_dir()?.into_os_string().into_vec();
    for arg in args {
        request.push(0);
        request.extend(arg.into_vec());
    }
    // Pass the standard streams so Pacman still draws progress on Paru's terminal.
    let (stdin, stdout, stderr) = (io::stdin(), io::stdout(), io::stderr());
    let fds = [stdin.as_fd(), stdout.as_fd(), stderr.as_fd()];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(3))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    control.push(SendAncillaryMessage::ScmRights(&fds));
    let sent = sendmsg(
        &stream,
        &[IoSlice::new(&request)],
        &mut control,
        SendFlags::empty(),
    )?;
    (&stream).write_all(&request[sent..])?;
    stream.shutdown(Shutdown::Write)?;

    let mut code = [0; 4];
    (&stream)
        .read_exact(&mut code)
        .context("waiting for pkexec")?;
    Ok(i32::from_le_bytes(code))
}

fn broker() -> Result<()> {
    let socket = socket()?;
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).context("binding the pkexec broker socket")?;
    // Linux applies the receive timeout to accept, so this ends once idle.
    set_socket_timeout(&listener, Timeout::Recv, Some(IDLE))?;
    while let Ok((stream, _)) = listener.accept() {
        let _ = serve(stream);
    }
    let _ = fs::remove_file(&socket);
    Ok(())
}

fn serve(stream: UnixStream) -> Result<()> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(3))];
    let mut control = RecvAncillaryBuffer::new(&mut space);
    let mut request = vec![0; 64 * 1024];
    let received = recvmsg(
        &stream,
        &mut [IoSliceMut::new(&mut request)],
        &mut control,
        RecvFlags::CMSG_CLOEXEC,
    )?;
    let mut fds = Vec::new();
    for message in control.drain() {
        if let RecvAncillaryMessage::ScmRights(rights) = message {
            fds.extend(rights);
        }
    }
    let [stdin, stdout, stderr]: [OwnedFd; 3] = fds
        .try_into()
        .ok()
        .context("missing the caller's standard streams")?;
    request.truncate(received.bytes);
    (&stream).read_to_end(&mut request)?;

    let mut parts = request
        .split(|&byte| byte == 0)
        .map(|part| OsString::from_vec(part.to_vec()));
    let directory = parts.next().unwrap_or_default();
    let status = Command::new("pkexec")
        .args(parts)
        .current_dir(directory)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .status()
        .context("starting pkexec")?;
    let code = status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
    (&stream).write_all(&code.to_le_bytes())?;
    Ok(())
}
