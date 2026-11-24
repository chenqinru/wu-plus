use crate::svn::RpcError;
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
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
    let name = if cfg!(windows) { "svn.exe" } else { "svn" };
    let from_path = std::env::split_paths(&path)
        .filter(|path| path.is_absolute())
        .map(|path| path.join(name));
    from_path
        .chain(system_svn_paths())
        .find(|path| is_svn_executable(path))
        .ok_or_else(|| RpcError::new("svn_unavailable", "Install Subversion 1.14 or newer."))
}

#[cfg(target_os = "macos")]
fn system_svn_paths() -> impl Iterator<Item = PathBuf> {
    ["/opt/homebrew/bin/svn", "/usr/local/bin/svn"]
        .into_iter()
        .map(PathBuf::from)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_svn_paths() -> impl Iterator<Item = PathBuf> {
    ["/usr/bin/svn", "/usr/local/bin/svn"]
        .into_iter()
        .map(PathBuf::from)
}

#[cfg(windows)]
fn system_svn_paths() -> impl Iterator<Item = PathBuf> {
    ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .flat_map(|directory| {
            ["SlikSvn", "TortoiseSVN", "Subversion"]
                .map(|install| directory.join(install).join("bin").join("svn.exe"))
        })
}

#[cfg(unix)]
fn is_svn_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    // On macOS this executable is an Xcode stub, even when the tools are installed.
    (!cfg!(target_os = "macos") || path != Path::new("/usr/bin/svn"))
        && path
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn is_svn_executable(path: &Path) -> bool {
    path.is_file()
}

const OUTPUT_LIMIT: usize = 32 * 1024 * 1024;

pub(crate) fn command(args: &[OsString]) -> Result<Command, RpcError> {
    #[cfg(windows)]
    windows::validate_arguments(args)?;
    let mut command = Command::new(executable()?);
    command.args(args).env("LC_ALL", "C.UTF-8");
    Ok(command)
}

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
    let mut command = command(args)?;
    command
        .stdin(if password.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    #[cfg(unix)]
    let mut child = {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0).spawn()?
    };
    #[cfg(windows)]
    let (mut child, job) = windows::spawn(command)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RpcError::new("io", "Missing SVN output pipe."))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| RpcError::new("io", "Missing SVN error pipe."))?;
    let stdout = thread::spawn(move || drain(stdout));
    let stderr = thread::spawn(move || drain(stderr));

    let result = (|| {
        if let Some(password) = password {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| RpcError::new("io", "Missing SVN input pipe."))?;
            // SVN reads password input up to APR's native line ending.
            let newline: &[u8] = if cfg!(windows) { b"\r\n" } else { b"\n" };
            if let Err(error) = stdin
                .write_all(password.as_bytes())
                .and_then(|()| stdin.write_all(newline))
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
    #[cfg(windows)]
    drop(job);
    if let Err(error) = result {
        #[cfg(unix)]
        if unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGKILL) } != 0 {
            eprintln!(
                "Unable to stop SVN process group: {}",
                std::io::Error::last_os_error()
            );
        }
        stop_and_reap(&mut child);
        return Err(error);
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
        let message = diagnostic_text(&stderr);
        let message = match password {
            Some(secret) if !secret.is_empty() => message.replace(secret, "[redacted]"),
            _ => message.trim().to_owned(),
        };
        return Err(RpcError::svn(message, status.code()));
    }
    match String::from_utf8(stdout) {
        Ok(output) => Ok(output),
        #[cfg(windows)]
        Err(error)
            if !args.iter().any(|arg| arg == "--xml")
                && !args.first().is_some_and(|command| command == "cat") =>
        {
            windows::decode_output(error.as_bytes()).ok_or_else(binary_output)
        }
        Err(_) => Err(binary_output()),
    }
}

fn binary_output() -> RpcError {
    RpcError::new(
        "binary",
        "This output is binary or uses an unsupported text encoding.",
    )
}

fn diagnostic_text(output: &[u8]) -> String {
    #[cfg(windows)]
    if std::str::from_utf8(output).is_err()
        && let Some(text) = windows::decode_output(output)
    {
        return text;
    }
    String::from_utf8_lossy(output).into_owned()
}

fn stop_and_reap(child: &mut Child) {
    if let Err(error) = child.kill() {
        eprintln!("Unable to stop SVN process: {error}");
    }
    if let Err(error) = child.wait() {
        eprintln!("Unable to reap SVN process: {error}");
    }
}

#[cfg(windows)]
mod windows {
    use super::{Child, Command, OsString, RpcError, stop_and_reap};
    use std::{
        io,
        os::windows::{
            ffi::OsStrExt as _,
            io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle},
            process::CommandExt as _,
        },
        ptr,
    };
    use windows_sys::Win32::{
        Globalization::{
            CP_ACP, CP_UTF8, GetACP, MB_ERR_INVALID_CHARS, MultiByteToWideChar,
            WC_NO_BEST_FIT_CHARS, WideCharToMultiByte,
        },
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            Threading::CREATE_NO_WINDOW,
        },
    };

    pub(super) fn validate_arguments(args: &[OsString]) -> Result<(), RpcError> {
        let code_page = unsafe { GetACP() };
        if matches!(code_page, CP_UTF8 | 54936) {
            return Ok(());
        }
        for argument in args
            .iter()
            .filter(|argument| !argument.as_encoded_bytes().is_ascii())
        {
            let wide = argument.encode_wide().collect::<Vec<_>>();
            let mut replaced = 0;
            let size = unsafe {
                WideCharToMultiByte(
                    code_page,
                    WC_NO_BEST_FIT_CHARS,
                    wide.as_ptr(),
                    wide.len() as i32,
                    ptr::null_mut(),
                    0,
                    ptr::null(),
                    &mut replaced,
                )
            };
            if size == 0 || replaced != 0 {
                return Err(RpcError::new(
                    "svn_encoding",
                    "A path or argument contains characters outside the Windows system code page used by Subversion.",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn spawn(mut command: Command) -> io::Result<(Child, OwnedHandle)> {
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&information).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut child = command.creation_flags(CREATE_NO_WINDOW).spawn()?;
        // As with the IDE's process wrapper, assign immediately while SVN is starting up.
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0 {
            let error = io::Error::last_os_error();
            if !child.try_wait().is_ok_and(|status| status.is_some()) {
                stop_and_reap(&mut child);
                return Err(error);
            }
        }
        Ok((child, job))
    }

    pub(super) fn decode_output(output: &[u8]) -> Option<String> {
        let length = i32::try_from(output.len()).ok()?;
        let size = unsafe {
            MultiByteToWideChar(
                CP_ACP,
                MB_ERR_INVALID_CHARS,
                output.as_ptr(),
                length,
                ptr::null_mut(),
                0,
            )
        };
        if size <= 0 {
            return None;
        }
        let mut decoded = vec![0; size as usize];
        let written = unsafe {
            MultiByteToWideChar(
                CP_ACP,
                MB_ERR_INVALID_CHARS,
                output.as_ptr(),
                length,
                decoded.as_mut_ptr(),
                size,
            )
        };
        (written == size)
            .then(|| String::from_utf16(&decoded).ok())
            .flatten()
    }
}
