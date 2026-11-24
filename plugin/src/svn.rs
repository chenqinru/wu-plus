mod operations;
mod read;

use crate::{Cancellation, PROTOCOL_VERSION, process};
use roxmltree::Document;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions, TryLockError},
    io,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct RpcError {
    pub kind: &'static str,
    pub message: String,
    pub exit_code: Option<i32>,
}

impl RpcError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            exit_code: None,
        }
    }

    pub fn cancelled() -> Self {
        Self::new("cancelled", "The SVN operation was cancelled.")
    }

    pub fn svn(message: String, exit_code: Option<i32>) -> Self {
        let kind = if message.contains("E230001") {
            "certificate"
        } else if ["E170001", "E215004", "E170013: Authentication", "E175013"]
            .iter()
            .any(|code| message.contains(code))
        {
            "authentication"
        } else {
            "svn"
        };
        let message = if message.is_empty() {
            "SVN exited with an error.".to_owned()
        } else {
            message
        };
        Self {
            kind,
            message,
            exit_code,
        }
    }
}

impl From<io::Error> for RpcError {
    fn from(value: io::Error) -> Self {
        Self::new("io", value.to_string())
    }
}

type Result<T> = std::result::Result<T, RpcError>;

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new("invalid_params", message)
}

fn decode<T: for<'a> Deserialize<'a>>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|error| invalid(error.to_string()))
}

fn xml(output: &str) -> Result<Document<'_>> {
    Document::parse(output).map_err(|error| RpcError::new("svn_xml", error.to_string()))
}

fn text<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|child| child.has_tag_name(name))
        .and_then(|node| node.text())
}

fn peg(value: impl AsRef<std::ffi::OsStr>) -> OsString {
    let mut target = value.as_ref().to_os_string();
    target.push("@");
    target
}

fn literal_local_target(value: impl AsRef<std::ffi::OsStr>) -> OsString {
    let mut target = value.as_ref().to_os_string();
    // SVN searches for a peg revision before canonicalizing the path. A final
    // slash-delimited component keeps an embedded @ out of that search.
    if cfg!(windows) && target.as_encoded_bytes().contains(&b'@') {
        target.push("/.");
    }
    target
}

fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = dunce::canonicalize(path)?;
    #[cfg(windows)]
    if let Some(share) = path
        .to_str()
        .and_then(|path| path.strip_prefix(r"\\?\UNC\"))
    {
        return Ok(PathBuf::from(format!(r"\\{share}")));
    }
    Ok(path)
}

fn is_working_copy_component(part: &std::ffi::OsStr) -> bool {
    #[cfg(windows)]
    {
        return part.to_str().is_some_and(|part| {
            !part.eq_ignore_ascii_case(".svn") && !part.ends_with(['.', ' ']) && !part.contains(':')
        });
    }
    #[cfg(not(windows))]
    {
        part != ".svn"
    }
}

fn revision(value: &str) -> Result<&str> {
    if value == "HEAD" || (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())) {
        Ok(value)
    } else {
        Err(invalid("Use a numeric SVN revision or HEAD."))
    }
}

fn url(value: &str) -> Result<&str> {
    let (scheme, address) = value
        .split_once("://")
        .ok_or_else(|| invalid("Enter an absolute SVN URL."))?;
    let authority = address.split('/').next().unwrap_or_default();
    let valid_scheme = matches!(scheme, "file" | "svn" | "svn+ssh" | "http" | "https");
    let valid_authority = if let Some((user, host)) = authority.rsplit_once('@') {
        scheme == "svn+ssh"
            && !user.is_empty()
            && !host.is_empty()
            && user
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    } else {
        !authority.is_empty() || scheme == "file"
    };
    if !valid_scheme
        || !valid_authority
        || address.is_empty()
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(invalid(
            "Use a valid SVN URL without embedded credentials. Percent-encode spaces.",
        ));
    }
    Ok(value)
}

fn repository_target(copy: &WorkingCopy, address: &str) -> Result<OsString> {
    let address = url(address)?;
    let base = copy.repository_url.trim_end_matches('/');
    let suffix = address.strip_prefix(base);
    let inside = suffix.is_some_and(|rest| {
        (rest.is_empty() || rest.starts_with('/'))
            && rest.split('/').all(|segment| {
                let segment = segment
                    .to_ascii_lowercase()
                    .replace("%2e", ".")
                    .replace("%2f", "/");
                segment
                    .split('/')
                    .all(|component| !matches!(component, "." | ".."))
            })
    });
    if base.is_empty() || !inside {
        return Err(invalid("Use a URL in the selected SVN repository."));
    }
    Ok(peg(address))
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Auth {
    username: String,
    password: Option<String>,
    remember: bool,
}

impl Auth {
    fn password(&self, realm: Option<&str>) -> Result<Option<String>> {
        if self.username.contains(['\r', '\n', '\0']) {
            return Err(invalid("The SVN username must be on a single line."));
        }
        if self.username.is_empty() && self.password.is_some() {
            return Err(invalid("Enter a username with the password."));
        }
        if let Some(password) = &self.password {
            if password.len() > 4096 || password.contains(['\r', '\n', '\0']) {
                return Err(invalid(
                    "The SVN password must be a single line of at most 4096 bytes.",
                ));
            }
            return Ok(Some(password.clone()));
        }
        if !self.username.is_empty()
            && let Some(realm) = realm
        {
            #[cfg(target_os = "macos")]
            {
                return match security_framework::passwords::get_generic_password(
                    "Wu Plus Subversion",
                    &format!("{realm}\n{}", self.username),
                ) {
                    Ok(password) => String::from_utf8(password).map(Some).map_err(|_| {
                        RpcError::new(
                            "keychain",
                            "The saved SVN credential has an invalid encoding.",
                        )
                    }),
                    Err(error) if error.code() == -25300 => Ok(None),
                    Err(error) => Err(RpcError::new("keychain", error.to_string())),
                };
            }
            #[cfg(not(target_os = "macos"))]
            let _ = realm;
        }
        Ok(None)
    }

    fn remember(&self, realm: Option<&str>) -> Result<()> {
        if self.remember
            && let (Some(realm), Some(password)) = (realm, &self.password)
        {
            #[cfg(target_os = "macos")]
            security_framework::passwords::set_generic_password(
                "Wu Plus Subversion",
                &format!("{realm}\n{}", self.username),
                password.as_bytes(),
            )
            .map_err(|error| RpcError::new("keychain", error.to_string()))?;
            #[cfg(not(target_os = "macos"))]
            let _ = (realm, password);
        }
        Ok(())
    }
}

fn command(
    cwd: Option<&Path>,
    mut args: Vec<OsString>,
    targets: Vec<OsString>,
    auth: &Auth,
    realm: Option<&str>,
    cancellation: &Cancellation,
) -> Result<String> {
    args.push("--non-interactive".into());
    let mut password = None;
    if !auth.username.is_empty() || auth.password.is_some() {
        password = auth.password(realm)?;
        args.extend([
            "--username".into(),
            auth.username.as_str().into(),
            "--no-auth-cache".into(),
        ]);
        if password.is_some() {
            args.push("--password-from-stdin".into());
        }
    }
    if !targets.is_empty() {
        args.push("--".into());
        args.extend(targets);
    }
    process::run(
        cwd,
        &args,
        password.as_deref(),
        Duration::from_secs(300),
        cancellation,
    )
}

fn flags(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

#[derive(Debug, Serialize)]
struct WorkingCopy {
    root: PathBuf,
    url: String,
    repository_url: String,
    uuid: String,
    revision: String,
}

fn info(path: &Path, cancellation: &Cancellation) -> Result<WorkingCopy> {
    let output = command(
        None,
        flags(&["info", "--xml", "--depth", "empty"]),
        vec![peg(path)],
        &Auth::default(),
        None,
        cancellation,
    )?;
    let document = xml(&output)?;
    let entry = document
        .descendants()
        .find(|node| node.has_tag_name("entry"))
        .ok_or_else(|| invalid("SVN did not report a working copy."))?;
    let root = entry
        .descendants()
        .find(|node| node.has_tag_name("wcroot-abspath"))
        .and_then(|node| node.text())
        .ok_or_else(|| invalid("The target is outside a working copy."))?;
    let repository = entry
        .children()
        .find(|node| node.has_tag_name("repository"))
        .ok_or_else(|| invalid("The working copy has no repository information."))?;
    Ok(WorkingCopy {
        root: canonicalize(root)?,
        url: text(entry, "url").unwrap_or_default().to_owned(),
        repository_url: text(repository, "root").unwrap_or_default().to_owned(),
        uuid: text(repository, "uuid").unwrap_or_default().to_owned(),
        revision: entry.attribute("revision").unwrap_or_default().to_owned(),
    })
}

fn root_info(root: &Path, cancellation: &Cancellation) -> Result<WorkingCopy> {
    if !root.is_absolute() {
        return Err(invalid("An absolute SVN working copy root is required."));
    }
    let actual = canonicalize(root)?;
    let copy = info(&actual, cancellation)?;
    if copy.root != actual {
        return Err(invalid("Select a working copy root."));
    }
    Ok(copy)
}

fn ensure_expected_url(copy: &WorkingCopy, expected: Option<&str>) -> Result<()> {
    if expected.is_some_and(|expected| expected != copy.url) {
        return Err(invalid("The SVN working copy URL changed. Refresh before retrying."));
    }
    Ok(())
}

fn file_target(root: &Path, path: &str) -> Result<PathBuf> {
    if path == "." {
        return Ok(root.to_path_buf());
    }
    let relative = Path::new(path);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(part) if is_working_copy_component(part)))
    {
        return Err(invalid(
            "The selected path is outside the working copy or points into SVN metadata.",
        ));
    }
    let target = root.join(relative);
    // SVN can operate on a symlink itself. Only resolve ancestors of the final component.
    for parent in target.parent().into_iter().flat_map(Path::ancestors) {
        if parent == root {
            break;
        }
        match fs::symlink_metadata(parent) {
            Ok(metadata) if metadata.is_symlink() || parent.join(".svn").is_dir() => {
                return Err(invalid(
                    "The selected path crosses into a different working copy or a symlink.",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if target.join(".svn").is_dir()
        && !fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.is_symlink())
    {
        return Err(invalid("Select the nested working copy separately."));
    }
    Ok(target)
}

fn write_lock(root: &Path, cancellation: &Cancellation) -> Result<File> {
    let normalized = canonicalize(root)?;
    let locks_dir = std::env::temp_dir().join("wu-plus-subversion-locks");
    fs::create_dir_all(&locks_dir)?;
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    normalized.hash(&mut hasher);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(locks_dir.join(format!("{:016x}.lock", hasher.finish())))?;
    let deadline = Instant::now() + Duration::from_millis(150);
    loop {
        cancellation.check()?;
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => {
                return Err(RpcError::new(
                    "busy",
                    "Another SVN operation is running for this working copy.",
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

pub fn handle(method: &str, params: Value, cancellation: &Cancellation) -> Result<Value> {
    cancellation.check()?;
    match method {
        "initialize" => {
            let version = process::run(
                None,
                &flags(&["--version", "--quiet"]),
                None,
                Duration::from_secs(5),
                cancellation,
            )?;
            let mut fields = version.trim().split('.');
            let major = fields
                .next()
                .and_then(|field| field.parse::<u32>().ok())
                .unwrap_or(0);
            let minor = fields
                .next()
                .and_then(|field| field.parse::<u32>().ok())
                .unwrap_or(0);
            if (major, minor) < (1, 14) {
                return Err(RpcError::new(
                    "svn_version",
                    "Subversion requires SVN 1.14 or newer.",
                ));
            }
            Ok(json!({"protocol_version": PROTOCOL_VERSION, "svn_version": version.trim()}))
        }
        "discover" => read::discover(decode(params)?, cancellation),
        "status" => read::status(decode(params)?, cancellation),
        "read" => read::run(decode(params)?, cancellation),
        "write" => operations::run(decode(params)?, cancellation),
        _ => Err(RpcError::new("method_not_found", "Unknown SVN method.")),
    }
}

/// Run SVN's own interactive certificate prompt in the host's terminal.
pub fn verify_certificate(address: &str) -> Result<i32> {
    let address = url(address)?;
    if !address.starts_with("https://") {
        return Err(invalid(
            "Certificate verification requires an HTTPS SVN URL.",
        ));
    }
    let mut args = flags(&["info", "--force-interactive", "--"]);
    args.push(peg(address));
    let status = process::command(&args)?.status()?;
    Ok(status.code().unwrap_or(1))
}

/// Let Subversion present conflict-specific choices on the integrated terminal.
pub fn resolve_interactively(root: &Path, path: &str) -> Result<i32> {
    let cancellation = Cancellation::default();
    let copy = root_info(root, &cancellation)?;
    let _lock = write_lock(&copy.root, &cancellation)?;
    let target = file_target(&copy.root, path)?;
    let mut args = flags(&["resolve", "--depth", "empty", "--force-interactive", "--"]);
    args.push(peg(target));
    let status = process::command(&args)?.current_dir(&copy.root).status()?;
    Ok(status.code().unwrap_or(1))
}
