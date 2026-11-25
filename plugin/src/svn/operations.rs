use super::*;
use std::collections::BTreeSet;

#[derive(Deserialize)]
pub(super) struct Write {
    root: Option<PathBuf>,
    expected_url: Option<String>,
    #[serde(flatten)]
    operation: Operation,
    #[serde(default)]
    auth: Auth,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Operation {
    Checkout {
        url: String,
        destination: PathBuf,
        revision: Option<String>,
        #[serde(default)]
        with_externals: bool,
    },
    Update {
        revision: Option<String>,
        #[serde(default)]
        with_externals: bool,
    },
    Add {
        paths: Vec<String>,
    },
    Delete {
        paths: Vec<String>,
    },
    Move {
        path: String,
        destination: String,
    },
    Revert {
        paths: Vec<String>,
        #[serde(default)]
        recursive: bool,
    },
    Cleanup {},
    Commit {
        paths: Vec<String>,
        message: String,
    },
    CopyBranch {
        source: String,
        destination: String,
        message: String,
        revision: Option<String>,
    },
    Switch {
        url: String,
        revision: Option<String>,
    },
    Merge {
        source: String,
        revisions: Option<String>,
        #[serde(default)]
        dry_run: bool,
    },
    SetProperty {
        path: String,
        name: String,
        value: Option<String>,
    },
    Lock {
        paths: Vec<String>,
        message: Option<String>,
    },
    Unlock {
        paths: Vec<String>,
    },
    Changelist {
        paths: Vec<String>,
        name: Option<String>,
    },
    Resolve {
        path: String,
        accept: Accept,
        kind: Option<ConflictKind>,
        incoming_revision: Option<u64>,
        expected_working: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Accept {
    Working,
    Base,
    MineFull,
    TheirsFull,
    MineConflict,
    TheirsConflict,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ConflictKind {
    Text,
    Property,
    Tree,
    Mixed,
}

impl Accept {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Base => "base",
            Self::MineFull => "mine-full",
            Self::TheirsFull => "theirs-full",
            Self::MineConflict => "mine-conflict",
            Self::TheirsConflict => "theirs-conflict",
        }
    }
}

fn selected(root: &Path, paths: &[String]) -> Result<Vec<OsString>> {
    if paths.is_empty() {
        return Err(invalid("Select at least one path."));
    }
    if paths.len() > 2000 {
        return Err(invalid("Select up to 2,000 paths per operation."));
    }
    paths
        .iter()
        .map(|path| file_target(root, path).map(peg))
        .collect()
}

fn message(value: &str) -> Result<&str> {
    if value.trim().is_empty() {
        return Err(invalid("Enter a commit message."));
    }
    if value.len() > 32 * 1024 || value.contains('\0') {
        return Err(invalid(
            "The commit message is too large or contains an invalid character.",
        ));
    }
    Ok(value)
}

fn with_revision(mut args: Vec<OsString>, revision_value: Option<&str>) -> Result<Vec<OsString>> {
    if let Some(value) = revision_value {
        args.extend(["--revision".into(), revision(value)?.into()]);
    }
    Ok(args)
}

fn with_message(
    mut args: Vec<OsString>,
    value: &str,
    argument_file: &mut Option<tempfile::NamedTempFile>,
) -> Result<Vec<OsString>> {
    if cfg!(windows) {
        use std::io::Write as _;
        // Windows SVN decodes command-line arguments through the system code page.
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(value.as_bytes())?;
        args.extend([
            "--file".into(),
            file.path().as_os_str().into(),
            "--encoding".into(),
            "UTF-8".into(),
        ]);
        *argument_file = Some(file);
    } else {
        args.extend(["--message".into(), value.into()]);
    }
    Ok(args)
}

pub(super) fn commit_scope(
    copy: &WorkingCopy,
    paths: Vec<String>,
    cancellation: &Cancellation,
) -> Result<Vec<read::Change>> {
    if paths.len() > 2000 {
        return Err(invalid("Select up to 2,000 paths per operation."));
    }
    let changes = read::changes(copy, false, false, &Auth::default(), cancellation)?;
    let mut selected_paths = paths.into_iter().collect::<BTreeSet<_>>();
    if selected_paths.is_empty() {
        return Err(invalid("Select the changes to commit."));
    }
    for path in &selected_paths {
        let entry = changes
            .iter()
            .find(|entry| &entry.path == path)
            .ok_or_else(|| {
                invalid(format!(
                    "{path} is no longer an SVN change. Refresh the selection."
                ))
            })?;
        if !entry.committable() {
            return Err(invalid(format!(
                "{path} cannot be committed in its current state."
            )));
        }
        for linked in [entry.moved_from.as_ref(), entry.moved_to.as_ref()]
            .into_iter()
            .flatten()
        {
            if !selected_paths.contains(linked) {
                return Err(invalid(format!(
                    "Select both sides of the SVN move: {path} and {linked}."
                )));
            }
        }
    }
    // SVN requires the explicit addition of any newly added parent directories.
    for path in selected_paths.clone() {
        for parent in Path::new(&path).ancestors().skip(1) {
            let Some(parent) = parent.to_str().filter(|path| !path.is_empty()) else {
                break;
            };
            if changes
                .iter()
                .any(|entry| entry.path == parent && entry.item == "added" && !entry.copied)
            {
                selected_paths.insert(parent.to_owned());
            }
        }
    }
    let entries = changes
        .into_iter()
        .filter(|entry| selected_paths.contains(&entry.path))
        .collect::<Vec<_>>();
    if entries.len() > 2000 {
        return Err(invalid("Select up to 2,000 paths per operation."));
    }
    for entry in &entries {
        file_target(&copy.root, &entry.path)?;
    }
    Ok(entries)
}

fn ensure_file_external_excluded(
    copy: &WorkingCopy,
    targets: &[OsString],
    cancellation: &Cancellation,
) -> Result<()> {
    let changes = read::changes(copy, false, false, &Auth::default(), cancellation)?;
    for external in changes.iter().filter(|entry| entry.file_external) {
        let external_target = peg(file_target(&copy.root, &external.path)?);
        if targets.contains(&external_target) {
            return Err(invalid(format!(
                "{} is an SVN file external.",
                external.path
            )));
        }
    }
    Ok(())
}

fn ensure_new_move_path(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(_) => Err(invalid("Choose a new path within this working copy.")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
fn is_case_only_move(source: &Path, destination: &Path) -> Result<bool> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

    let (Some(parent), Some(source_name), Some(destination_name)) =
        (source.parent(), source.file_name(), destination.file_name())
    else {
        return Ok(false);
    };
    if destination.parent() != Some(parent) || source_name == destination_name {
        return Ok(false);
    }
    let source_wide = source_name.encode_wide().collect::<Vec<_>>();
    let destination_wide = destination_name.encode_wide().collect::<Vec<_>>();
    let comparison = unsafe {
        CompareStringOrdinal(
            source_wide.as_ptr(),
            source_wide.len() as i32,
            destination_wide.as_ptr(),
            destination_wide.len() as i32,
            1,
        )
    };
    if comparison == 0 {
        return Err(io::Error::last_os_error().into());
    }
    if comparison != CSTR_EQUAL {
        return Ok(false);
    }
    let mut found_source = false;
    for entry in fs::read_dir(parent)? {
        let name = entry?.file_name();
        if name == destination_name {
            return Ok(false);
        }
        found_source |= name == source_name;
    }
    Ok(found_source)
}

#[cfg(windows)]
fn move_with_new_case(
    copy: &WorkingCopy,
    source: &Path,
    destination: &Path,
    cancellation: &Cancellation,
) -> Result<Value> {
    ensure_file_external_excluded(copy, &[peg(source)], cancellation)?;
    let parent = source
        .parent()
        .ok_or_else(|| invalid("Choose a new path within this working copy."))?;
    let temporary = tempfile::Builder::new()
        .prefix(".wu-svn-move-")
        .tempfile_in(parent)?;
    let intermediate = temporary.path().to_path_buf();
    temporary.close()?;
    cancellation.check()?;

    // Keep the working-copy lock and complete the two moves even if the request is cancelled.
    let finish = Cancellation::default();
    let move_path = |from: &Path, to: &Path| {
        ensure_new_move_path(to)?;
        command(
            Some(&copy.root),
            flags(&["move"]),
            vec![peg(from), literal_local_target(to)],
            &Auth::default(),
            None,
            &finish,
        )
    };
    let move_result =
        move_path(source, &intermediate).and_then(|_| move_path(&intermediate, destination));
    match move_result {
        Ok(content) => Ok(json!({"text":content,"warning":null})),
        Err(error) => {
            let recovery = match intermediate.symlink_metadata() {
                Ok(_) => move_path(&intermediate, source),
                Err(recovery) if recovery.kind() == io::ErrorKind::NotFound => return Err(error),
                Err(recovery) => Err(recovery.into()),
            };
            match recovery {
                Ok(_) => Err(error),
                Err(recovery) => Err(RpcError::new(
                    "uncertain",
                    format!(
                        "{} Could not restore {} from {}: {} Refresh and verify the working copy.",
                        error.message,
                        source.display(),
                        intermediate.display(),
                        recovery.message,
                    ),
                )),
            }
        }
    }
}

fn check_write_authentication(
    target: OsString,
    revision: Option<&str>,
    auth: &Auth,
    realm: &str,
    cancellation: &Cancellation,
) -> Result<()> {
    if target
        .to_str()
        .is_some_and(|target| target.starts_with("file://"))
    {
        return Ok(());
    }
    let args = with_revision(flags(&["info", "--xml", "--depth", "empty"]), revision)?;
    match command(None, args, vec![target], auth, Some(realm), cancellation) {
        Ok(_) => Ok(()),
        Err(mut error) if error.kind == "authentication" => {
            error.kind = "authentication_before_write";
            Err(error)
        }
        // The write can have different path permissions or use a different peg revision.
        Err(error) if error.kind == "svn" => Ok(()),
        Err(error) => Err(error),
    }
}

fn write_outcome(error: RpcError) -> RpcError {
    let interrupted = error.kind == "svn"
        && ["E170013", "E175002", "E205011", "E730054", "E000104"]
            .iter()
            .any(|code| error.message.contains(code));
    if matches!(
        error.kind,
        "timeout" | "cancelled" | "io" | "authentication" | "binary" | "output_limit"
    ) || interrupted {
        return RpcError::new(
            "uncertain",
            format!(
                "{} Refresh and verify the repository before retrying.",
                error.message
            ),
        );
    }
    error
}

fn checkout(
    address: String,
    destination: PathBuf,
    rev: Option<&str>,
    with_externals: bool,
    auth: &Auth,
    cancellation: &Cancellation,
) -> Result<Value> {
    let address = url(&address)?;
    let name = destination
        .file_name()
        .ok_or_else(|| invalid("Choose a checkout folder."))?;
    let parent = destination
        .parent()
        .filter(|path| path.is_absolute())
        .ok_or_else(|| invalid("Choose an absolute checkout destination."))?;
    let parent = canonicalize(parent)?;
    let destination = parent.join(name);
    let _lock = write_lock(&parent, cancellation)?;
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if !metadata.is_dir() || fs::read_dir(&destination)?.next().is_some() => {
            return Err(invalid(
                "The checkout destination must be absent or an empty directory.",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut args = flags(&["checkout"]);
    if !with_externals {
        args.push("--ignore-externals".into());
    }
    let args = with_revision(args, rev)?;
    check_write_authentication(peg(address), rev, auth, address, cancellation)?;
    let result = command(
        Some(&parent),
        args,
        vec![peg(address), literal_local_target(&destination)],
        auth,
        Some(address),
        cancellation,
    )
    .map_err(write_outcome)?;
    let warning = auth
        .remember(Some(address))
        .and_then(|_| {
            if auth.remember && auth.password.is_some() {
                let copy = root_info(&destination, &Cancellation::default())?;
                if copy.repository_url != address {
                    auth.remember(Some(&copy.repository_url))?;
                }
            }
            Ok(())
        })
        .err()
        .map(|error| error.message);
    Ok(json!({"text":result,"destination":destination,"warning":warning}))
}

pub(super) fn run(params: Write, cancellation: &Cancellation) -> Result<Value> {
    let auth = params.auth;
    let action = match params.operation {
        Operation::Checkout {
            url,
            destination,
            revision,
            with_externals,
        } => {
            return checkout(
                url,
                destination,
                revision.as_deref(),
                with_externals,
                &auth,
                cancellation,
            );
        }
        action => action,
    };
    let root = params
        .root
        .ok_or_else(|| invalid("Select an SVN working copy."))?;
    if !root.is_absolute() {
        return Err(invalid("An absolute SVN working copy root is required."));
    }
    let _lock = write_lock(&root, cancellation)?;
    let copy = root_info(&root, cancellation)?;
    ensure_expected_url(&copy, params.expected_url.as_deref())?;
    let mut argument_file = None;
    let (args, targets, touches_server) = match action {
        Operation::Update {
            revision,
            with_externals,
        } => {
            let mut args = flags(&["update", "--accept", "postpone"]);
            if !with_externals {
                args.push("--ignore-externals".into());
            }
            (
                with_revision(args, revision.as_deref())?,
                vec![peg(&copy.root)],
                true,
            )
        }
        Operation::Add { paths } => (
            flags(&["add", "--parents"]),
            selected(&copy.root, &paths)?,
            false,
        ),
        Operation::Delete { paths } => (flags(&["delete"]), selected(&copy.root, &paths)?, false),
        Operation::Move { path, destination } => {
            let source = file_target(&copy.root, &path)?;
            let destination = file_target(&copy.root, &destination)?;
            if source == copy.root || destination == copy.root || source == destination {
                return Err(invalid("Choose a new path within this working copy."));
            }
            #[cfg(windows)]
            if is_case_only_move(&source, &destination)? {
                return move_with_new_case(&copy, &source, &destination, cancellation);
            }
            ensure_new_move_path(&destination)?;
            (
                flags(&["move"]),
                vec![peg(source), literal_local_target(destination)],
                false,
            )
        }
        Operation::Revert { paths, recursive } => (
            flags(&[
                "revert",
                "--depth",
                if recursive { "infinity" } else { "empty" },
            ]),
            selected(&copy.root, &paths)?,
            false,
        ),
        Operation::Cleanup {} => (flags(&["cleanup"]), vec![peg(&copy.root)], false),
        Operation::Commit {
            paths,
            message: value,
        } => (
            with_message(
                flags(&["commit", "--depth", "empty"]),
                message(&value)?,
                &mut argument_file,
            )?,
            commit_scope(&copy, paths, cancellation)?
                .into_iter()
                .map(|entry| peg(copy.root.join(entry.path)))
                .collect(),
            true,
        ),
        Operation::CopyBranch {
            source,
            destination,
            message: value,
            revision,
        } => (
            with_revision(
                with_message(flags(&["copy"]), message(&value)?, &mut argument_file)?,
                revision.as_deref(),
            )?,
            vec![
                repository_target(&copy, &source)?,
                url(&destination).and_then(|address| {
                    repository_target(&copy, address).map(|_| OsString::from(address))
                })?,
            ],
            true,
        ),
        Operation::Switch { url, revision } => (
            with_revision(
                flags(&["switch", "--ignore-externals", "--accept", "postpone"]),
                revision.as_deref(),
            )?,
            vec![
                repository_target(&copy, &url)?,
                literal_local_target(&copy.root),
            ],
            true,
        ),
        Operation::Merge {
            source,
            revisions,
            dry_run,
        } => {
            let mut args = flags(&["merge", "--accept", "postpone"]);
            if dry_run {
                args.push("--dry-run".into());
            }
            if let Some(revisions) = revisions {
                let pieces = revisions
                    .split_once(':')
                    .ok_or_else(|| invalid("Enter a merge range such as 12:16."))?;
                revision(pieces.0)?;
                revision(pieces.1)?;
                args.extend(["--revision".into(), revisions.into()]);
            }
            (
                args,
                vec![
                    repository_target(&copy, &source)?,
                    literal_local_target(&copy.root),
                ],
                true,
            )
        }
        Operation::SetProperty { path, name, value } => {
            if name.is_empty() || name.starts_with('-') || name.chars().any(char::is_whitespace) {
                return Err(invalid("Enter a valid SVN property name."));
            }
            let path = peg(file_target(&copy.root, &path)?);
            if let Some(value) = value {
                use std::io::Write as _;
                let mut file = tempfile::NamedTempFile::new()?;
                file.write_all(value.as_bytes())?;
                let args = vec![
                    "propset".into(),
                    "--file".into(),
                    file.path().as_os_str().into(),
                    name.into(),
                ];
                argument_file = Some(file);
                (args, vec![path], false)
            } else {
                (vec!["propdel".into(), name.into()], vec![path], false)
            }
        }
        Operation::Lock { paths, message } => {
            let mut args = flags(&["lock"]);
            if let Some(message) = message {
                args = with_message(args, &message, &mut argument_file)?;
            }
            (args, selected(&copy.root, &paths)?, true)
        }
        Operation::Unlock { paths } => (flags(&["unlock"]), selected(&copy.root, &paths)?, true),
        Operation::Changelist { paths, name } => {
            let mut args = flags(&["changelist"]);
            if let Some(name) = name {
                if name.trim().is_empty() || name.starts_with('-') {
                    return Err(invalid("Enter a valid changelist name."));
                }
                args.push(name.into());
            } else {
                args.push("--remove".into());
            }
            (args, selected(&copy.root, &paths)?, false)
        }
        Operation::Resolve {
            path,
            accept,
            kind,
            incoming_revision,
            expected_working,
        } => {
            let target = file_target(&copy.root, &path)?;
            let status =
                read::status_entries(&copy, &target, false, false, &Auth::default(), cancellation)?;
            let entry = status
                .iter()
                .find(|entry| entry.path == path && entry.conflict())
                .ok_or_else(|| invalid("This path has no unresolved SVN conflict."))?;
            if let Some(kind) = kind {
                let text = entry.item == "conflicted";
                let property = entry.props == "conflicted";
                let tree = entry.tree_conflicted;
                let actual = match (text, property, tree) {
                    (true, false, false) => ConflictKind::Text,
                    (false, true, false) => ConflictKind::Property,
                    (false, false, true) => ConflictKind::Tree,
                    _ => ConflictKind::Mixed,
                };
                if kind != actual {
                    return Err(invalid(
                        "The SVN conflict changed. Reopen it before resolving.",
                    ));
                }
            }
            let args = if kind == Some(ConflictKind::Tree) && accept == Accept::TheirsFull {
                let output = command(
                    Some(&copy.root),
                    flags(&["info", "--xml"]),
                    vec![peg(&target)],
                    &Auth::default(),
                    None,
                    cancellation,
                )?;
                let info = xml(&output)?;
                let version = read::incoming_tree_add(&info)
                    .and_then(|incoming| incoming.attribute("revision"))
                    .and_then(|revision| revision.parse::<u64>().ok());
                if version.is_none()
                    || version != incoming_revision
                    || entry.item != "replaced"
                    || entry.file_external
                    || expected_working.is_none()
                    || read::read_small_text(&target) != expected_working
                {
                    return Err(invalid(
                        "The incoming file or local replacement changed. Reopen the SVN conflict.",
                    ));
                }
                // For update/add/add, SVN has already installed the incoming
                // node underneath the local replacement. Revert just that node.
                flags(&["revert", "--depth", "empty"])
            } else {
                if matches!(kind, Some(ConflictKind::Tree | ConflictKind::Mixed))
                    && accept != Accept::Working
                {
                    return Err(invalid(
                        "Review all changes before resolving this SVN conflict.",
                    ));
                }
                if entry.item == "conflicted"
                    && accept == Accept::Working
                    && let Some(content) = read::read_small_text(&target)
                    && content.lines().any(|line| {
                        line.starts_with("<<<<<<< ")
                            || line.starts_with("||||||| ")
                            || line.starts_with(">>>>>>> ")
                    })
                {
                    return Err(invalid(
                        "Remove all conflict markers and save before marking the file resolved.",
                    ));
                }
                flags(&["resolve", "--depth", "empty", "--accept", accept.as_str()])
            };
            (args, vec![peg(&target)], false)
        }
        Operation::Checkout { .. } => unreachable!(),
    };
    if matches!(
        args.first().and_then(|s| s.to_str()),
        Some(
            "add"
                | "delete"
                | "move"
                | "revert"
                | "propset"
                | "propdel"
                | "lock"
                | "unlock"
                | "changelist"
                | "resolve"
        )
    ) {
        ensure_file_external_excluded(&copy, &targets, cancellation)?;
    }
    let local = Auth::default();
    let effective_auth = if touches_server { &auth } else { &local };
    let writes_to_server = touches_server && !args.iter().any(|argument| argument == "--dry-run");
    if writes_to_server {
        let target = if matches!(
            args.first().and_then(|arg| arg.to_str()),
            Some("copy" | "switch" | "merge")
        ) {
            targets.first().cloned().unwrap_or_else(|| peg(&copy.url))
        } else {
            peg(&copy.url)
        };
        let revision = args
            .windows(2)
            .find_map(|pair| {
                (pair[0] == "--revision")
                    .then(|| pair[1].to_str())
                    .flatten()
            })
            .filter(|revision| !revision.contains(':'));
        check_write_authentication(
            target,
            revision,
            effective_auth,
            &copy.repository_url,
            cancellation,
        )?;
    }
    let output = command(
        Some(&copy.root),
        args,
        targets,
        effective_auth,
        Some(&copy.repository_url),
        cancellation,
    );
    drop(argument_file);
    match output {
        Ok(content) => {
            let warning = effective_auth
                .remember(Some(&copy.repository_url))
                .err()
                .map(|error| error.message);
            Ok(json!({"text":content,"warning":warning}))
        }
        Err(error) if writes_to_server => Err(write_outcome(error)),
        Err(error) => Err(error),
    }
}
