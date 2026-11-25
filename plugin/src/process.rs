use crate::svn::RpcError;
use std::{
    collections::HashSet,
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, OnceLock,
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

struct Client {
    path: PathBuf,
    version: String,
    #[cfg(windows)]
    utf8: bool,
}

static CLIENT: OnceLock<Client> = OnceLock::new();

fn executable_candidates(path: &std::ffi::OsStr) -> impl Iterator<Item = PathBuf> + '_ {
    let name = if cfg!(windows) { "svn.exe" } else { "svn" };
    let from_path = std::env::split_paths(path)
        .filter(|path| path.is_absolute())
        .map(move |path| path.join(name));
    #[cfg(windows)]
    let from_path = bundled_client().into_iter().chain(from_path);
    from_path.chain(system_svn_paths())
}

#[cfg(windows)]
fn bundled_client() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    Some(executable.parent()?.join("subversion").join("svn.exe"))
}

fn client(cancellation: &Cancellation) -> Result<&'static Client, RpcError> {
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let selection = select_client(executable_candidates(&path), cancellation)?;
    Ok(CLIENT.get_or_init(|| selection))
}

fn select_client(
    candidates: impl IntoIterator<Item = PathBuf>,
    cancellation: &Cancellation,
) -> Result<Client, RpcError> {
    let mut visited = HashSet::new();
    let mut rejected = Vec::new();
    let arguments = ["--version".into(), "--quiet".into()];
    #[cfg(windows)]
    let bundled = bundled_client();
    for path in candidates {
        cancellation.check()?;
        if !visited.insert(path.clone()) || !is_svn_executable(&path) {
            continue;
        }
        let mut client = Client {
            #[cfg(windows)]
            utf8: bundled.as_ref() == Some(&path) && windows::supports_utf8_manifest(),
            path,
            version: String::new(),
        };
        let result = run_client(
            &client,
            None,
            &arguments,
            None,
            Duration::from_secs(5),
            cancellation,
        );
        match result {
            Ok(output) => {
                let version = output.trim();
                let mut fields = version.split('.');
                let major = fields.next().and_then(|field| field.parse::<u32>().ok());
                let minor = fields.next().and_then(|field| field.parse::<u32>().ok());
                if matches!(major.zip(minor), Some(version) if version >= (1, 14)) {
                    client.version = version.to_owned();
                    return Ok(client);
                }
                rejected.push(format!("{} ({version})", client.path.display()));
            }
            Err(error) if error.kind == "cancelled" => return Err(error),
            Err(error) => rejected.push(format!("{} ({})", client.path.display(), error.message)),
        }
    }
    if rejected.is_empty() {
        return Err(RpcError::new(
            "svn_unavailable",
            "Install Subversion 1.14 or newer.",
        ));
    }
    Err(RpcError::new(
        "svn_version",
        format!(
            "Subversion requires SVN 1.14 or newer. Checked {}.",
            rejected.join("; ")
        ),
    ))
}

pub fn version(cancellation: &Cancellation) -> Result<&'static str, RpcError> {
    Ok(&client(cancellation)?.version)
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
    client(&Cancellation::default())?.command(args)
}

impl Client {
    fn command(&self, args: &[OsString]) -> Result<Command, RpcError> {
        #[cfg(windows)]
        if !self.utf8 {
            windows::validate_arguments(args)?;
        }
        let mut command = Command::new(&self.path);
        command.args(args).env("LC_ALL", "C.UTF-8");
        Ok(command)
    }
}

#[cfg(windows)]
pub(crate) fn write_target_list(
    targets: &[OsString],
    writer: &mut impl Write,
    cancellation: &Cancellation,
) -> Result<(), RpcError> {
    let client = client(cancellation)?;
    for target in targets {
        let target = target.to_str().ok_or_else(binary_output)?;
        if target.contains(['\r', '\n', '\0']) {
            return Err(RpcError::new("invalid_params", "Invalid SVN target path."));
        }
        if client.utf8 {
            writer.write_all(target.as_bytes())?;
        } else {
            writer.write_all(&windows::encode_argument(target.as_ref())?)?;
        }
        writer.write_all(b"\n")?;
    }
    Ok(())
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
    run_client(
        client(cancellation)?,
        cwd,
        args,
        password,
        timeout,
        cancellation,
    )
}

fn run_client(
    client: &Client,
    cwd: Option<&Path>,
    args: &[OsString],
    password: Option<&str>,
    timeout: Duration,
    cancellation: &Cancellation,
) -> Result<String, RpcError> {
    cancellation.check()?;
    let mut command = client.command(args)?;
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
            if !client.utf8
                && !args.iter().any(|arg| arg == "--xml")
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt as _};

    #[test]
    fn selects_the_first_compatible_subversion_client() {
        let directory = tempfile::tempdir().unwrap();
        let paths = ["broken", "old", "compatible"].map(|name| directory.path().join(name));
        for (path, script) in paths.iter().zip([
            "#!/bin/sh\nexit 1\n",
            "#!/bin/sh\necho 1.9.7\n",
            "#!/bin/sh\necho 1.14.5\n",
        ]) {
            fs::write(path, script).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let selected = select_client(paths.clone(), &Cancellation::default()).unwrap();
        assert_eq!(selected.path, paths[2]);
        let error = select_client(paths[..2].iter().cloned(), &Cancellation::default())
            .err()
            .expect("all clients were rejected");
        assert_eq!(error.kind, "svn_version");
        assert!(error.message.contains("1.9.7"));
    }
}

#[cfg(windows)]
mod windows {
    use super::{Child, Command, OsString, RpcError, stop_and_reap};
    use std::{
        ffi::OsStr,
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
            SystemInformation::OSVERSIONINFOW,
            Threading::CREATE_NO_WINDOW,
        },
    };

    pub(super) fn supports_utf8_manifest() -> bool {
        use windows_sys::Wdk::System::SystemServices::RtlGetVersion;

        let mut version = OSVERSIONINFOW {
            dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        let status = unsafe { RtlGetVersion(&mut version) };
        status == 0 && (version.dwMajorVersion, version.dwBuildNumber) >= (10, 18362)
    }

    pub(super) fn validate_arguments(args: &[OsString]) -> Result<(), RpcError> {
        for argument in args
            .iter()
            .filter(|argument| !argument.as_encoded_bytes().is_ascii())
        {
            encode_argument(argument)?;
        }
        Ok(())
    }

    pub(super) fn encode_argument(argument: &OsStr) -> Result<Vec<u8>, RpcError> {
        if argument.as_encoded_bytes().is_ascii() {
            return Ok(argument.as_encoded_bytes().to_vec());
        }
        let code_page = unsafe { GetACP() };
        let wide = argument.encode_wide().collect::<Vec<_>>();
        let mut encoded = vec![0; wide.len() * 4];
        let mut replaced = 0;
        let (flags, replaced_ptr) = if matches!(code_page, CP_UTF8 | 54936) {
            (0, ptr::null_mut())
        } else {
            (WC_NO_BEST_FIT_CHARS, &mut replaced as *mut _)
        };
        let written = unsafe {
            WideCharToMultiByte(
                code_page,
                flags,
                wide.as_ptr(),
                wide.len() as i32,
                encoded.as_mut_ptr(),
                encoded.len() as i32,
                ptr::null(),
                replaced_ptr,
            )
        };
        if written == 0 || replaced != 0 {
            return Err(RpcError::new(
                "svn_encoding",
                "This SVN client cannot represent the path or argument with the Windows system code page. Use the Subversion client bundled with Wu Plus.",
            ));
        }
        encoded.truncate(written as usize);
        Ok(encoded)
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
