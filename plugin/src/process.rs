use crate::svn::RpcError;
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn check(&self) -> Result<(), RpcError> {
        if self.0.load(Ordering::Relaxed) {
            return Err(RpcError::cancelled());
        }
        Ok(())
    }
}

pub fn executable() -> Result<PathBuf, RpcError> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let from_path = std::env::split_paths(&path)
        .filter(|path| path.is_absolute())
        .map(|path| path.join("svn"));
    from_path
        .chain(["/opt/homebrew/bin/svn", "/usr/local/bin/svn"].map(PathBuf::from))
        .find(|path| {
            use std::os::unix::fs::PermissionsExt as _;
            path.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            }) && path != Path::new("/usr/bin/svn")
        })
        .ok_or_else(|| {
            RpcError::new(
                "svn_unavailable",
                "Install Subversion 1.14 or newer (brew install subversion).",
            )
        })
}

const OUTPUT_LIMIT: usize = 32 * 1024 * 1024;

fn drain(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut buffer = [0; 8192];
    let mut output = Vec::new();
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(output);
        }
        if output.len() <= OUTPUT_LIMIT {
            output.extend_from_slice(&buffer[..count]);
        }
    }
}

pub fn run(
    cwd: Option<&Path>,
    args: &[OsString],
    password: Option<&str>,
    timeout: Duration,
    cancellation: &Cancellation,
) -> Result<String, RpcError> {
    cancellation.check()?;
    let mut command = Command::new(executable()?);
    command
        .args(args)
        .env("LC_ALL", "C.UTF-8")
        .stdin(if password.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout = thread::spawn(move || drain(stdout));
    let stderr = thread::spawn(move || drain(stderr));

    let result = (|| {
        if let Some(password) = password {
            let mut stdin = child.stdin.take().expect("piped stdin");
            // The password is bounded and only crosses the child's stdin.
            if let Err(error) = writeln!(stdin, "{password}")
                && error.kind() != std::io::ErrorKind::BrokenPipe
            {
                return Err(error.into());
            }
        }
        let start = Instant::now();
        let mut poll_interval = Duration::from_millis(1);
        loop {
            if let Some(status) = child.try_wait()? {
                break Ok(status);
            }
            cancellation.check()?;
            if start.elapsed() >= timeout {
                break Err(RpcError::new("timeout", "The SVN operation timed out."));
            }
            thread::sleep(poll_interval);
            poll_interval = (poll_interval * 2).min(Duration::from_millis(25));
        }
    })();
    if result.is_err() {
        // Terminate the entire process group so an SSH child cannot keep the pipes open.
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
        let _ = child.wait();
    }
    let status = result?;
    let stdout = stdout
        .join()
        .map_err(|_| RpcError::new("io", "Unable to read SVN output."))??;
    let stderr = stderr
        .join()
        .map_err(|_| RpcError::new("io", "Unable to read SVN errors."))??;
    if stdout.len() > OUTPUT_LIMIT || stderr.len() > OUTPUT_LIMIT {
        return Err(RpcError::new(
            "output_limit",
            "SVN returned too much output. Narrow the file or revision scope.",
        ));
    }
    if !status.success() {
        let message = String::from_utf8_lossy(&stderr);
        let message = match password {
            Some(secret) if !secret.is_empty() => message.replace(secret, "[redacted]"),
            _ => message.trim().to_owned(),
        };
        return Err(RpcError::svn(message, status.code()));
    }
    String::from_utf8(stdout).map_err(|_| {
        RpcError::new(
            "binary",
            "This output is binary or uses an unsupported text encoding.",
        )
    })
}
