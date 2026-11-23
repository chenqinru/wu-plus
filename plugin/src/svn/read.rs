use super::*;
use std::collections::{BTreeMap, HashSet};

const CONFLICT_ARTIFACTS: [(&str, &str); 4] = [
    ("prev-base-file", "base"),
    ("prev-wc-file", "mine"),
    ("cur-base-file", "incoming"),
    ("prop-file", "property_reject"),
];

#[derive(Deserialize)]
pub(super) struct Discovery {
    roots: Vec<PathBuf>,
}

pub(super) fn discover(params: Discovery, cancellation: &Cancellation) -> Result<Value> {
    const MAX_DIRECTORIES: usize = 75_000;
    let mut pending = Vec::new();
    let mut copies = BTreeMap::new();
    let mut visited = HashSet::new();
    let mut warnings = Vec::new();
    for root in params.roots {
        cancellation.check()?;
        if !root.is_absolute() {
            return Err(invalid("Workspace roots must be absolute paths."));
        }
        let root = match fs::canonicalize(&root) {
            Ok(path) => path,
            Err(error) => {
                warnings.push(format!("{}: {error}", root.display()));
                continue;
            }
        };
        let dir = if root.is_dir() {
            root.as_path()
        } else {
            root.parent().unwrap_or(&root)
        };
        if let Some(parent) = dir.ancestors().find(|dir| dir.join(".svn").is_dir()) {
            insert_copy(parent, &mut copies, &mut warnings, cancellation)?;
        }
        if root.is_dir() {
            pending.push(root);
        }
    }
    while let Some(dir) = pending.pop() {
        cancellation.check()?;
        if !visited.insert(dir.clone()) {
            continue;
        }
        if visited.len() > MAX_DIRECTORIES {
            warnings.push("The SVN discovery scan reached its limit. Add a specific folder to the workspace to scan it directly.".into());
            break;
        }
        if dir.join(".svn").is_dir() {
            insert_copy(&dir, &mut copies, &mut warnings, cancellation)?;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                if warnings.len() < 20 {
                    warnings.push(format!("{}: {error}", dir.display()));
                }
                continue;
            }
        };
        for entry in entries.flatten() {
            if matches!(
                entry.file_name().to_str(),
                Some(".svn" | ".git" | ".hg" | "node_modules")
            ) {
                continue;
            }
            if entry.file_type().is_ok_and(|metadata| metadata.is_dir()) {
                pending.push(entry.path());
            }
        }
    }
    Ok(json!({"copies": copies.into_values().collect::<Vec<_>>(), "warnings": warnings}))
}

fn insert_copy(
    path: &Path,
    copies: &mut BTreeMap<PathBuf, WorkingCopy>,
    warnings: &mut Vec<String>,
    cancellation: &Cancellation,
) -> Result<()> {
    if copies.contains_key(path) {
        return Ok(());
    }
    match root_info(path, cancellation) {
        Ok(copy) => {
            copies.insert(copy.root.clone(), copy);
        }
        Err(error) if error.kind == "cancelled" => return Err(error),
        Err(error) => warnings.push(format!("{}: {}", path.display(), error.message)),
    }
    Ok(())
}

#[derive(Deserialize)]
pub(super) struct Status {
    root: PathBuf,
    #[serde(default)]
    remote: bool,
    #[serde(default)]
    ignored: bool,
    #[serde(default)]
    diff_stats: bool,
    #[serde(default)]
    auth: Auth,
}

#[derive(Clone, Serialize)]
pub(super) struct Change {
    pub(super) path: String,
    pub(super) is_dir: bool,
    pub(super) item: String,
    pub(super) props: String,
    pub(super) tree_conflicted: bool,
    pub(super) file_external: bool,
    pub(super) copied: bool,
    pub(super) switched: bool,
    pub(super) locked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    lock: Option<FileLock>,
    pub(super) remote_item: Option<String>,
    pub(super) remote_props: Option<String>,
    pub(super) changelist: Option<String>,
    pub(super) moved_from: Option<String>,
    pub(super) moved_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff_stat: Option<DiffStat>,
}

#[derive(Clone, Serialize)]
struct FileLock {
    owner: Option<String>,
    comment: Option<String>,
    held_here: bool,
}

#[derive(Clone, Copy, Default, Serialize)]
struct DiffStat {
    added: usize,
    deleted: usize,
}

impl Change {
    pub(super) fn conflict(&self) -> bool {
        self.tree_conflicted || self.item == "conflicted" || self.props == "conflicted"
    }

    pub(super) fn committable(&self) -> bool {
        !self.conflict()
            && !self.file_external
            && (matches!(
                self.item.as_str(),
                "added" | "modified" | "deleted" | "replaced"
            ) || (self.item == "normal" && self.props == "modified"))
    }
}

fn relative(root: &Path, path: &str) -> Option<String> {
    let path = Path::new(path);
    let path = if path.is_absolute() {
        path.strip_prefix(root).ok()?
    } else {
        path
    };
    if path.as_os_str().is_empty() || path == Path::new(".") {
        return Some(".".into());
    }
    if path
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return path.to_str().map(str::to_owned);
    }
    None
}

fn move_relative(root: &Path, target: &Path, value: &str) -> Option<String> {
    let path = Path::new(value);
    if path.is_absolute() {
        return relative(root, value);
    }
    let mut path_from_root = target.strip_prefix(root).ok()?.to_path_buf();
    for component in path.components() {
        match component {
            Component::ParentDir if !path_from_root.pop() => return None,
            Component::ParentDir | Component::CurDir => {}
            Component::Normal(name) => path_from_root.push(name),
            _ => return None,
        }
    }
    relative(root, path_from_root.to_str()?)
}

pub(super) fn changes(
    copy: &WorkingCopy,
    remote: bool,
    ignored: bool,
    auth: &Auth,
    cancellation: &Cancellation,
) -> Result<Vec<Change>> {
    status_entries(copy, &copy.root, remote, ignored, auth, cancellation)
}

pub(super) fn status_entries(
    copy: &WorkingCopy,
    target: &Path,
    remote: bool,
    ignored: bool,
    auth: &Auth,
    cancellation: &Cancellation,
) -> Result<Vec<Change>> {
    let mut args = flags(&["status", "--xml", "--ignore-externals"]);
    if target != copy.root {
        args.extend(flags(&["--depth", "empty"]));
    }
    if remote {
        args.push("--show-updates".into());
    }
    if ignored {
        args.push("--no-ignore".into());
    }
    let output = command(
        Some(&copy.root),
        args,
        vec![peg(target)],
        auth,
        Some(&copy.repository_url),
        cancellation,
    )?;
    let document = xml(&output)?;
    let mut changes = Vec::new();
    for entry in document
        .descendants()
        .filter(|node| node.has_tag_name("entry"))
    {
        let Some(path) = entry
            .attribute("path")
            .and_then(|path| relative(&copy.root, path))
        else {
            continue;
        };
        let Some(status) = entry.children().find(|node| node.has_tag_name("wc-status")) else {
            continue;
        };
        let repos_status = entry
            .children()
            .find(|node| node.has_tag_name("repos-status"));
        let local_lock = status.children().find(|node| node.has_tag_name("lock"));
        let repos_lock =
            repos_status.and_then(|node| node.children().find(|node| node.has_tag_name("lock")));
        let lock = repos_lock
            .or_else(|| local_lock.filter(|_| !remote))
            .map(|node| FileLock {
                owner: text(node, "owner").map(str::to_owned),
                comment: text(node, "comment").map(str::to_owned),
                held_here: local_lock
                    .and_then(|local| text(local, "token"))
                    .is_some_and(|token| Some(token) == text(node, "token")),
            });
        changes.push(Change {
            is_dir: fs::symlink_metadata(copy.root.join(&path))
                .is_ok_and(|metadata| metadata.is_dir()),
            path,
            item: status.attribute("item").unwrap_or_default().into(),
            props: status.attribute("props").unwrap_or_default().into(),
            tree_conflicted: status.attribute("tree-conflicted") == Some("true"),
            file_external: status.attribute("file-external") == Some("true"),
            copied: status.attribute("copied") == Some("true"),
            switched: status.attribute("switched") == Some("true"),
            locked: lock.is_some(),
            lock,
            remote_item: repos_status
                .and_then(|node| node.attribute("item"))
                .filter(|item| *item != "none")
                .map(str::to_owned),
            remote_props: repos_status
                .and_then(|node| node.attribute("props"))
                .filter(|item| *item != "none")
                .map(str::to_owned),
            changelist: entry
                .ancestors()
                .find(|node| node.has_tag_name("changelist"))
                .and_then(|node| node.attribute("name"))
                .map(str::to_owned),
            moved_from: status
                .attribute("moved-from")
                .and_then(|path| move_relative(&copy.root, target, path)),
            moved_to: status
                .attribute("moved-to")
                .and_then(|path| move_relative(&copy.root, target, path)),
            diff_stat: None,
        });
    }
    hide_conflict_artifacts(copy, &mut changes, cancellation)?;
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

fn hide_conflict_artifacts(
    copy: &WorkingCopy,
    entries: &mut Vec<Change>,
    cancellation: &Cancellation,
) -> Result<()> {
    if !entries.iter().any(|entry| entry.item == "unversioned") {
        return Ok(());
    }
    let paths = entries
        .iter()
        .filter(|entry| entry.item == "conflicted" || entry.props == "conflicted")
        .map(|entry| peg(copy.root.join(&entry.path)))
        .collect::<Vec<_>>();
    let mut artifacts = HashSet::new();
    for paths in paths.chunks(64) {
        let output = match command(
            Some(&copy.root),
            flags(&["info", "--xml", "--depth", "empty"]),
            paths.to_vec(),
            &Auth::default(),
            None,
            cancellation,
        ) {
            Ok(output) => output,
            Err(error) if error.kind == "cancelled" => return Err(error),
            Err(_) => continue,
        };
        if let Ok(document) = xml(&output) {
            artifacts.extend(
                document
                    .descendants()
                    .filter(|node| {
                        CONFLICT_ARTIFACTS
                            .iter()
                            .any(|(tag, _)| node.has_tag_name(*tag))
                    })
                    .filter_map(|node| node.text().and_then(|path| relative(&copy.root, path))),
            );
        }
    }
    entries.retain(|entry| entry.item != "unversioned" || !artifacts.contains(&entry.path));
    Ok(())
}

pub(super) fn status(params: Status, cancellation: &Cancellation) -> Result<Value> {
    let copy = root_info(&params.root, cancellation)?;
    let local = Auth::default();
    let auth = if params.remote { &params.auth } else { &local };
    let mut entries = changes(&copy, params.remote, params.ignored, auth, cancellation)?;
    if params.diff_stats {
        set_diff_stats(&copy, &mut entries, cancellation)?;
    }
    let warning = auth
        .remember(Some(&copy.repository_url))
        .err()
        .map(|error| error.message);
    Ok(json!({"copy":copy,"entries":entries,"warning":warning}))
}

fn set_diff_stats(
    copy: &WorkingCopy,
    entries: &mut [Change],
    cancellation: &Cancellation,
) -> Result<()> {
    use std::io::Read as _;
    const MAX_STAT_FILES: usize = 250;
    if entries.is_empty() || entries.len() > MAX_STAT_FILES {
        return Ok(());
    }
    let diff = command(
        Some(&copy.root),
        flags(&["diff", "--internal-diff", "--ignore-properties"]),
        Vec::new(),
        &Auth::default(),
        None,
        cancellation,
    );
    let mut stats = match diff {
        Ok(diff) => patch_line_counts(&diff)
            .filter_map(|(path, stats)| relative(&copy.root, path).map(|path| (path, stats)))
            .collect::<BTreeMap<_, _>>(),
        Err(error) if error.kind == "cancelled" => return Err(error),
        Err(_) => BTreeMap::new(),
    };
    for entry in entries
        .iter_mut()
        .filter(|entry| !entry.is_dir && !entry.conflict() && !entry.file_external)
    {
        cancellation.check()?;
        entry.diff_stat = stats
            .remove(&entry.path)
            .filter(|stat| stat.added + stat.deleted > 0);
        if entry.item != "unversioned" {
            continue;
        }
        let Ok(path) = file_target(&copy.root, &entry.path) else {
            continue;
        };
        if !fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= 64 * 1024)
        {
            continue;
        }
        let mut contents = String::new();
        if fs::File::open(&path)
            .and_then(|file| file.take(64 * 1024 + 1).read_to_string(&mut contents))
            .is_ok()
            && contents.len() <= 64 * 1024
            && !contents.is_empty()
            && !contents.contains('\0')
        {
            entry.diff_stat = Some(DiffStat {
                added: contents.lines().count(),
                deleted: 0,
            });
        }
    }
    Ok(())
}

fn patch_line_counts(diff: &str) -> impl Iterator<Item = (&str, DiffStat)> {
    diff.split("\nIndex: ")
        .enumerate()
        .filter_map(|(index, section)| {
            let section = if index == 0 {
                section.strip_prefix("Index: ")?
            } else {
                section
            };
            let (path, changes) = section.split_once('\n')?;
            let mut stats = DiffStat::default();
            let mut in_hunk = false;
            for line in changes.lines() {
                if line.starts_with("@@ ") {
                    in_hunk = true;
                } else if in_hunk {
                    match line.as_bytes().first() {
                        Some(b'+') => stats.added += 1,
                        Some(b'-') => stats.deleted += 1,
                        _ => {}
                    }
                }
            }
            Some((path.trim_end_matches('\r'), stats))
        })
}

#[derive(Deserialize)]
pub(super) struct Read {
    root: PathBuf,
    #[serde(flatten)]
    operation: Operation,
    #[serde(default)]
    auth: Auth,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Operation {
    Diff {
        path: String,
        revision: Option<String>,
    },
    EditorBase {
        path: String,
    },
    WorktreeDiff {
        focus_path: Option<String>,
    },
    CommitDiff {
        paths: Vec<String>,
    },
    History {
        path: String,
        before: Option<String>,
    },
    RevisionReview {
        revision: u64,
        #[serde(default)]
        include_files: bool,
        focus_path: Option<String>,
    },
    Blame {
        path: String,
        revision: Option<String>,
        #[serde(default)]
        editor: bool,
    },
    Properties {
        path: String,
    },
    Branches {
        url: Option<String>,
    },
    SwitchTargets,
    Conflict {
        path: String,
    },
}

pub(super) fn run(params: Read, cancellation: &Cancellation) -> Result<Value> {
    let copy = root_info(&params.root, cancellation)?;
    if let Operation::Conflict { path } = &params.operation {
        return conflict(&copy, path, &params.auth, cancellation);
    }
    let needs_server = match &params.operation {
        Operation::Diff { revision, .. } => revision.is_some(),
        Operation::History { .. }
        | Operation::RevisionReview { .. }
        | Operation::Blame { .. }
        | Operation::Branches { .. }
        | Operation::SwitchTargets => true,
        Operation::WorktreeDiff { .. }
        | Operation::EditorBase { .. }
        | Operation::CommitDiff { .. }
        | Operation::Properties { .. }
        | Operation::Conflict { .. } => false,
    };
    let auth = if needs_server {
        params.auth
    } else {
        Auth::default()
    };
    let invoke = |args: Vec<OsString>, targets: Vec<OsString>| {
        command(
            Some(&copy.root),
            args,
            targets,
            &auth,
            Some(&copy.repository_url),
            cancellation,
        )
    };
    let warning = || {
        auth.remember(Some(&copy.repository_url))
            .err()
            .map(|error| error.message)
    };
    let content = match params.operation {
        Operation::EditorBase { path } => {
            let target = file_target(&copy.root, &path)?;
            if read_small_text(&target).is_none_or(|file| file.contains('\0')) {
                return Ok(json!({"base": null}));
            }
            let base = match invoke(flags(&["cat", "-r", "BASE"]), vec![peg(&target)]) {
                Ok(base) if base.len() <= MAX_DIFF_TEXT && !base.contains('\0') => Some(base),
                Ok(_) => None,
                Err(error) if matches!(error.kind, "binary" | "output_limit") => None,
                Err(error) if error.kind == "svn" => {
                    let changes =
                        status_entries(&copy, &target, false, false, &auth, cancellation)?;
                    changes
                        .iter()
                        .any(|entry| entry.path == path && entry.item == "added" && !entry.copied)
                        .then(String::new)
                }
                Err(error) => return Err(error),
            };
            return Ok(json!({"base": base}));
        }
        Operation::Diff {
            path,
            revision: None,
        } => {
            let target = file_target(&copy.root, &path)?;
            let status = status_entries(&copy, &target, false, false, &auth, cancellation)?;
            return working_diff(
                &copy,
                &path,
                status.iter().find(|entry| entry.path == path),
                cancellation,
            );
        }
        Operation::WorktreeDiff { focus_path } => {
            return worktree_diff(&copy, focus_path.as_deref(), cancellation);
        }
        Operation::CommitDiff { paths } => return commit_diff(&copy, paths, cancellation),
        Operation::Diff {
            path,
            revision: Some(rev),
        } => {
            let target = file_target(&copy.root, &path)?;
            let rev = revision(&rev)?;
            let resolved;
            let rev = if rev == "HEAD" {
                resolved = invoke(
                    flags(&["info", "--show-item", "revision", "-r", "HEAD"]),
                    vec![peg(&copy.repository_url)],
                )?;
                resolved.trim()
            } else {
                rev
            };
            let args = flags(&["diff", "--internal-diff", "--change", rev]);
            let patch = invoke(args, vec![peg(&target)])?;
            let mut response = json!({"path":path,"text":patch,"warning":warning()});
            if let Ok(rev) = rev.parse::<u64>()
                && rev > 0
            {
                let before = (rev - 1).to_string();
                let after = rev.to_string();
                let from = format!("r{before}");
                let to = format!("r{after}");
                if target.is_file() {
                    let cat = |revision: &str| {
                        invoke(flags(&["cat", "-r", revision]), vec![peg(&target)])
                    };
                    if let (Ok(base), Ok(head)) = (cat(&before), cat(&after)) {
                        response["file"] = diff_pair(&path, base, head, from.clone(), to.clone());
                    }
                }
                if patch_has_properties(&patch) && !target.is_dir() {
                    response["details"] = property_diff(&path, from, to, |revision| {
                        read_properties(
                            &invoke,
                            peg(&target),
                            Some(if revision { &after } else { &before }),
                        )
                    })?;
                }
            }
            response["reason"] = diff_reason(&patch, target.is_dir()).into();
            return Ok(response);
        }
        Operation::History { path, before } => {
            let from = before
                .as_deref()
                .map(revision)
                .transpose()?
                .unwrap_or("HEAD");
            let args = flags(&[
                "log",
                "--xml",
                "--verbose",
                "--limit",
                "50",
                "-r",
                &format!("{from}:0"),
            ]);
            let output = invoke(args, vec![peg(file_target(&copy.root, &path)?)])?;
            let mut response = format_history(&output)?;
            response["warning"] = json!(warning());
            return Ok(response);
        }
        Operation::RevisionReview {
            revision,
            include_files,
            focus_path,
        } => {
            let mut review = revision_review(
                &copy,
                revision,
                include_files,
                focus_path.as_deref(),
                &invoke,
                cancellation,
            )?;
            review["warning"] = json!(warning());
            return Ok(review);
        }
        Operation::Blame {
            path,
            revision: rev,
            editor,
        } => {
            let target = file_target(&copy.root, &path)?;
            let rev = rev.as_deref().map(revision).transpose()?;
            let args = |format| {
                let mut args = flags(&["blame", format]);
                if let Some(rev) = rev {
                    args.extend(["-r".into(), rev.into()]);
                }
                args
            };
            let content = if let Some(rev) = rev {
                invoke(flags(&["cat", "-r", rev]), vec![peg(&target)])?
            } else {
                read_small_text(&target).unwrap_or_default()
            };
            let blame = if !content.is_empty()
                && content.len() <= MAX_DIFF_TEXT
                && !content.contains('\0')
            {
                let output = invoke(args("--xml"), vec![peg(&target)])?;
                if editor && rev.is_none() && read_small_text(&target).as_deref() != Some(&content)
                {
                    return Ok(json!({"blame":null}));
                }
                format_blame(&path, content, &output)?
            } else {
                Value::Null
            };
            let annotated_text = if editor || !blame.is_null() {
                String::new()
            } else {
                invoke(args("--verbose"), vec![peg(&target)])?
            };
            return Ok(json!({"text":annotated_text,"blame":blame,"warning":warning()}));
        }
        Operation::Properties { path } => invoke(
            flags(&["proplist", "--verbose"]),
            vec![peg(file_target(&copy.root, &path)?)],
        )?,
        Operation::Branches { url: address } => {
            let address = address.as_deref().unwrap_or(&copy.repository_url);
            let target = repository_target(&copy, address)?;
            invoke(flags(&["list", "--verbose"]), vec![target])?
        }
        Operation::SwitchTargets => {
            let mut result = switch_targets(&copy, &invoke)?;
            result["warning"] = json!(warning());
            return Ok(result);
        }
        Operation::Conflict { .. } => unreachable!(),
    };
    Ok(json!({"text":content,"warning":warning()}))
}

fn switch_targets(
    copy: &WorkingCopy,
    invoke: &impl Fn(Vec<OsString>, Vec<OsString>) -> Result<String>,
) -> Result<Value> {
    let repository = copy.repository_url.trim_end_matches('/');
    let relative = copy
        .url
        .strip_prefix(repository)
        .unwrap_or_default()
        .trim_matches('/');
    let parts = relative.split('/').collect::<Vec<_>>();
    let standard = parts
        .iter()
        .position(|part| matches!(*part, "trunk" | "branches" | "tags"));
    if standard.is_none() && !relative.is_empty() {
        return Ok(json!({"targets":[{"path":relative,"url":copy.url}]}));
    }
    let (layout, suffix) = standard.map_or_else(
        || (repository.to_owned(), String::new()),
        |index| {
            let layout = if index == 0 {
                repository.to_owned()
            } else {
                format!("{repository}/{}", parts[..index].join("/"))
            };
            // A checkout of trunk/service should offer branches of that same subtree.
            let consumed = if parts[index] == "trunk" { 1 } else { 2 };
            let suffix = parts.get(index + consumed..).unwrap_or_default().join("/");
            (layout, suffix)
        },
    );
    let list_dirs = |address: &str| -> Result<Vec<String>> {
        let output = invoke(
            flags(&["list", "--xml", "--depth", "immediates"]),
            vec![repository_target(copy, address)?],
        )?;
        let document = xml(&output)?;
        Ok(document
            .descendants()
            .filter(|entry| entry.has_tag_name("entry") && entry.attribute("kind") == Some("dir"))
            .filter_map(|entry| text(entry, "name"))
            .filter(|name| !name.is_empty() && !name.contains('/') && !matches!(*name, "." | ".."))
            .map(str::to_owned)
            .collect())
    };
    let mut rows = Vec::new();
    let mut push_target = |path: String, mut address: String| {
        if !suffix.is_empty() {
            address.push('/');
            address.push_str(&suffix);
        }
        rows.push(json!({"path":path,"url":address}));
    };
    let directories = list_dirs(&layout)?;
    for group in ["trunk", "branches", "tags"] {
        if !directories.iter().any(|directory| directory == group) {
            continue;
        }
        let address = format!("{layout}/{group}");
        if group == "trunk" {
            push_target(group.into(), address);
            continue;
        }
        for name in list_dirs(&address)? {
            push_target(
                format!("{group}/{name}"),
                encode_url_path(format!("{address}/"), &name),
            );
        }
    }
    if let Some(current) = rows.iter_mut().find(|row| {
        row["url"]
            .as_str()
            .is_some_and(|address| url_bytes(address).eq(url_bytes(&copy.url)))
    }) {
        current["url"] = json!(copy.url);
    } else {
        rows.insert(0, json!({"path":relative,"url":copy.url}));
    }
    Ok(json!({"targets":rows}))
}

const MAX_DIFF_TEXT: usize = 4 * 1024 * 1024;

pub(super) fn read_small_text(path: &Path) -> Option<String> {
    use std::io::Read as _;
    if !fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_DIFF_TEXT as u64)
    {
        return None;
    }
    let mut contents = String::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_DIFF_TEXT as u64 + 1)
        .read_to_string(&mut contents)
        .ok()?;
    (contents.len() <= MAX_DIFF_TEXT).then_some(contents)
}

fn format_blame(path: &str, content: String, xml_output: &str) -> Result<Value> {
    let document = xml(xml_output)?;
    let mut lines: Vec<Value> = Vec::new();
    for entry in document
        .descendants()
        .filter(|node| node.has_tag_name("entry"))
    {
        let commit = entry.children().find(|node| node.has_tag_name("commit"));
        let revision = commit.and_then(|commit| commit.attribute("revision"));
        if let Some(line) = entry
            .attribute("line-number")
            .and_then(|line| line.parse::<u32>().ok())
            && let (Some(revision), Some(commit)) = (revision, commit)
        {
            if let Some(previous) = lines.last_mut()
                && previous["revision"].as_str() == Some(revision)
                && previous["end_line"]
                    .as_u64()
                    .and_then(|end| end.checked_add(1))
                    == Some(u64::from(line))
            {
                previous["end_line"] = json!(line);
            } else {
                lines.push(json!({"line":line,"end_line":line,"revision":revision,"author":text(commit,"author").unwrap_or_default(),"date":text(commit,"date").unwrap_or_default()}));
            }
        }
    }
    Ok(json!({"path":path,"content":content,"lines":lines}))
}

fn diff_pair(path: &str, base: String, head: String, from: String, to: String) -> Value {
    let is_text = |contents: &str| {
        contents.chars().all(|character| {
            !character.is_control() || matches!(character, '\r' | '\n' | '\t' | '\u{000C}')
        })
    };
    if head == base
        || base.len() > MAX_DIFF_TEXT
        || head.len() > MAX_DIFF_TEXT
        || !is_text(&base)
        || !is_text(&head)
    {
        return Value::Null;
    }
    json!({"path":path,"base":base,"head":head,"from":from,"to":to})
}

fn commit_diff(
    copy: &WorkingCopy,
    paths: Vec<String>,
    cancellation: &Cancellation,
) -> Result<Value> {
    use std::fmt::Write as _;

    const MAX_BYTES: usize = 20_000;
    const TRUNCATED: &str = "\n[SVN diff truncated]\n";

    let entries = operations::commit_scope(copy, paths, cancellation)?;
    let auth = Auth::default();
    let invoke =
        |args, targets| command(Some(&copy.root), args, targets, &auth, None, cancellation);
    let mut output = String::from("Selected SVN changes:\n");
    for entry in &entries {
        if let Some(source) = &entry.moved_from {
            let _ = writeln!(output, "moved {source} -> {}", entry.path);
        } else if entry.moved_to.is_none() {
            let kind = if entry.is_dir { "directory" } else { "path" };
            let _ = write!(output, "{} {kind} {}", entry.item, entry.path);
            if entry.props == "modified" {
                output.push_str(" (properties modified)");
            }
            if entry.copied {
                output.push_str(" (copied)");
            }
            output.push('\n');
        }
        if output.len() >= MAX_BYTES {
            break;
        }
    }
    let mut file_targets = Vec::new();
    for entry in &entries {
        if output.len() >= MAX_BYTES {
            break;
        }
        cancellation.check()?;
        // SVN may descend into an added directory even with --depth empty in a
        // diff. Read the selected directory's properties directly.
        let directory = if entry.is_dir {
            true
        } else if entry.item == "deleted" {
            match invoke(
                flags(&["info", "--show-item", "kind", "-r", "BASE"]),
                vec![peg(&entry.path)],
            ) {
                Ok(kind) => kind.trim() != "file",
                Err(error) if error.kind == "svn" => true,
                Err(error) => return Err(error),
            }
        } else {
            false
        };
        if !directory {
            file_targets.push(OsString::from(&entry.path));
        } else if entry.props == "modified" {
            let details = property_diff(&entry.path, "BASE".into(), "WORKING".into(), |head| {
                if !head && entry.item == "added" && !entry.copied {
                    return Ok(BTreeMap::new());
                }
                read_properties(&invoke, peg(&entry.path), (!head).then_some("BASE"))
            })?;
            if let (Some(base), Some(head)) = (details["base"].as_str(), details["head"].as_str()) {
                let _ = writeln!(output, "\nProperty changes on: {}", entry.path);
                for line in base.lines() {
                    let _ = writeln!(output, "-{line}");
                }
                for line in head.lines() {
                    let _ = writeln!(output, "+{line}");
                }
            }
        }
    }
    for batch in file_targets.chunks(32) {
        if output.len() >= MAX_BYTES {
            break;
        }
        let patch = invoke(
            flags(&["diff", "--internal-diff", "--depth", "empty"]),
            batch.to_vec(),
        );
        match patch {
            Ok(patch) if !patch.is_empty() => {
                output.push('\n');
                output.push_str(&patch);
            }
            Err(error) if matches!(error.kind, "binary" | "output_limit") => {
                output.push_str("\n[Text diff unavailable for part of the selection]\n");
            }
            Err(error) => return Err(error),
            Ok(_) => {}
        }
    }
    if output.len() >= MAX_BYTES {
        output.truncate(output.floor_char_boundary(MAX_BYTES - TRUNCATED.len()));
        output.push_str(TRUNCATED);
    }
    Ok(json!({"text":output}))
}

fn worktree_diff(
    copy: &WorkingCopy,
    focus_path: Option<&str>,
    cancellation: &Cancellation,
) -> Result<Value> {
    const MAX_FILES: usize = 100;
    const MAX_RESPONSE_TEXT: usize = 12 * 1024 * 1024;
    let auth = Auth::default();
    let entries = if let Some(path) = focus_path {
        let target = file_target(&copy.root, path)?;
        status_entries(copy, &target, false, false, &auth, cancellation)?
    } else {
        changes(copy, false, false, &auth, cancellation)?
    };
    let mut files = Vec::new();
    let mut other = Vec::new();
    let mut size = 0;
    let mut inspected = 0;
    let mut limit_reached = false;
    for entry in entries.iter().filter(|entry| {
        focus_path.is_none_or(|path| path == entry.path)
            && !entry.file_external
            && !entry.conflict()
            && (entry.committable() || matches!(entry.item.as_str(), "missing" | "unversioned"))
    }) {
        cancellation.check()?;
        if entry.moved_to.is_some() {
            continue;
        }
        if inspected >= MAX_FILES || size >= MAX_RESPONSE_TEXT {
            limit_reached = true;
            other.push(&entry.path);
            continue;
        }
        inspected += 1;
        let diff = working_diff(copy, &entry.path, Some(entry), cancellation)?;
        let mut included = false;
        for file in [&diff["file"], &diff["details"]] {
            if file.is_null() {
                continue;
            }
            let bytes = ["base", "head"]
                .iter()
                .map(|key| file[*key].as_str().map_or(0, str::len))
                .sum::<usize>();
            if size + bytes <= MAX_RESPONSE_TEXT {
                size += bytes;
                files.push(file.clone());
                included = true;
            } else {
                limit_reached = true;
            }
        }
        if !included {
            other.push(&entry.path);
        }
    }
    Ok(json!({"files":files,"other":other,"limited":limit_reached}))
}

fn working_diff(
    copy: &WorkingCopy,
    path: &str,
    status: Option<&Change>,
    cancellation: &Cancellation,
) -> Result<Value> {
    let target = file_target(&copy.root, path)?;
    let auth = Auth::default();
    let invoke = |args: Vec<OsString>, targets: Vec<OsString>| {
        command(Some(&copy.root), args, targets, &auth, None, cancellation)
    };
    let moved_from = status.and_then(|entry| entry.moved_from.as_deref());
    let moved_to = status.and_then(|entry| entry.moved_to.as_deref());
    let before = moved_from.unwrap_or(path);
    let after = moved_to.unwrap_or(path);
    let moved = before != after;
    let base_target = file_target(&copy.root, before)?;
    let head_target = file_target(&copy.root, after)?;
    let metadata = fs::symlink_metadata(&head_target).ok();
    let directory = metadata.as_ref().is_some_and(fs::Metadata::is_dir)
        || status.is_some_and(|entry| entry.is_dir);
    let unversioned = status.is_some_and(|entry| entry.item == "unversioned");
    let removed =
        !moved && status.is_some_and(|entry| matches!(entry.item.as_str(), "deleted" | "missing"));
    let added = !moved && status.is_some_and(|entry| entry.item == "added" && !entry.copied);
    let patch = if unversioned || directory {
        String::new()
    } else {
        match invoke(
            flags(&["diff", "--internal-diff", "--depth", "empty"]),
            vec![target.into_os_string()],
        ) {
            Ok(patch) => patch,
            Err(error) if matches!(error.kind, "binary" | "output_limit") => String::new(),
            Err(error) => return Err(error),
        }
    };
    let possible_text = !directory
        && (removed
            || metadata.as_ref().is_some_and(|metadata| {
                metadata.is_file() && metadata.len() <= MAX_DIFF_TEXT as u64
            }));
    let (file, unavailable) =
        if possible_text && !patch.contains("Cannot display: file marked as a binary type") {
            let head = if removed {
                Some(String::new())
            } else {
                read_small_text(&head_target)
            };
            let base = if added || unversioned {
                Some(String::new())
            } else {
                match invoke(flags(&["cat", "-r", "BASE"]), vec![peg(&base_target)]) {
                    Ok(base) => Some(base),
                    Err(error) if matches!(error.kind, "binary" | "output_limit" | "svn") => None,
                    Err(error) => return Err(error),
                }
            };
            match (base, head) {
                (Some(base), Some(head)) => {
                    let differs = base != head;
                    let file = diff_pair(after, base, head, "BASE".into(), "WORKING".into());
                    let unavailable = differs && file.is_null();
                    (file, unavailable)
                }
                _ => (Value::Null, true),
            }
        } else {
            (Value::Null, !directory)
        };
    let properties_changed = patch_has_properties(&patch)
        || status.is_some_and(|entry| matches!(entry.props.as_str(), "modified" | "conflicted"))
        || moved;
    let mut details = if properties_changed && !unversioned {
        property_diff(after, "BASE".into(), "WORKING".into(), |head| {
            if (head && removed) || (!head && added) {
                return Ok(BTreeMap::new());
            }
            if head {
                read_properties(&invoke, peg(&head_target), None)
            } else {
                read_properties(&invoke, peg(&base_target), Some("BASE"))
            }
        })?
    } else {
        Value::Null
    };
    if details.is_null() && (moved || directory && (added || removed)) {
        let base = if added {
            String::new()
        } else {
            format!("{before}\n")
        };
        let head = if removed {
            String::new()
        } else {
            format!("{after}\n")
        };
        details = diff_pair(after, base, head, "BASE".into(), "WORKING".into());
        details["kind"] = "path".into();
    }
    let reason = if unavailable {
        "binary"
    } else {
        diff_reason(&patch, directory)
    };
    Ok(json!({"path":after,"text":patch,"file":file,"details":details,"reason":reason}))
}

fn revision_file(
    copy: &WorkingCopy,
    path: &str,
    rev: u64,
    action: &str,
    directory: bool,
    invoke: &impl Fn(Vec<OsString>, Vec<OsString>) -> Result<String>,
) -> Result<Value> {
    if rev == 0 || !matches!(action, "A" | "D" | "M" | "R") {
        return Err(invalid("Select a file change in an SVN revision."));
    }
    let address = history_file_url(copy, path)?;
    let before = rev - 1;
    let content = |rev: u64| {
        invoke(
            flags(&["cat", "-r", &rev.to_string()]),
            vec![format!("{address}@{rev}").into()],
        )
    };
    let base = if directory || action == "A" {
        Ok(String::new())
    } else {
        content(before)
    };
    let head = if directory || action == "D" {
        Ok(String::new())
    } else {
        content(rev)
    };
    let peg_revision = if action == "D" { before } else { rev };
    let patch = || {
        invoke(
            flags(&[
                "diff",
                "--internal-diff",
                "--depth",
                "empty",
                "--change",
                &rev.to_string(),
            ]),
            vec![format!("{address}@{peg_revision}").into()],
        )
    };
    let patch = patch()?;
    let from = format!("r{before}");
    let to = format!("r{rev}");
    let mut details = if patch_has_properties(&patch) {
        property_diff(path, from.clone(), to.clone(), |head| {
            if (head && action == "D") || (!head && action == "A") {
                return Ok(BTreeMap::new());
            }
            let revision = if head { rev } else { before }.to_string();
            read_properties(
                invoke,
                format!("{address}@{revision}").into(),
                Some(&revision),
            )
        })?
    } else {
        Value::Null
    };
    if details.is_null() && directory && matches!(action, "A" | "D" | "R") {
        let path_line = format!("{path}\n");
        let base = if action == "A" {
            String::new()
        } else {
            path_line.clone()
        };
        let head = if action == "D" {
            String::new()
        } else {
            path_line
        };
        details = diff_pair(path, base, head, from.clone(), to.clone());
        if !details.is_null() {
            details["kind"] = "path".into();
        }
    }
    let reason = diff_reason(&patch, directory);
    if reason == "binary" {
        return Ok(json!({"path":path,"details":details,"text":patch,"reason":reason}));
    }
    match (base, head) {
        (Ok(base), Ok(head)) => {
            let file = diff_pair(path, base, head, from, to);
            Ok(json!({"path":path,"file":file,"details":details,"text":patch,"reason":reason}))
        }
        (Err(error), _) | (_, Err(error)) if matches!(error.kind, "binary" | "output_limit") => {
            Ok(json!({"path":path,"details":details,"text":patch,"reason":"binary"}))
        }
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn revision_review(
    copy: &WorkingCopy,
    rev: u64,
    include_files: bool,
    focus_path: Option<&str>,
    invoke: &impl Fn(Vec<OsString>, Vec<OsString>) -> Result<String>,
    cancellation: &Cancellation,
) -> Result<Value> {
    if rev == 0 {
        return Err(invalid("Select an SVN revision."));
    }
    let rev_text = rev.to_string();
    let target = || vec![format!("{}@{rev}", copy.repository_url).into()];
    let focus_path = focus_path
        .filter(|_| include_files)
        .map(|path| path.trim_start_matches('/'));
    let stats = if focus_path.is_some() {
        None
    } else {
        match invoke(
            flags(&[
                "diff",
                "--internal-diff",
                "--ignore-properties",
                "--change",
                &rev_text,
            ]),
            target(),
        ) {
            Ok(diff) => Some(patch_line_counts(&diff).fold(
                DiffStat::default(),
                |mut total, (_, stat)| {
                    total.added += stat.added;
                    total.deleted += stat.deleted;
                    total
                },
            )),
            Err(error) if matches!(error.kind, "output_limit" | "binary") => None,
            Err(error) => return Err(error),
        }
    };
    if !include_files {
        return Ok(json!({"revision":rev,"stats":stats}));
    }

    let log = invoke(
        flags(&["log", "--xml", "--verbose", "-r", &rev_text]),
        target(),
    )?;
    let doc = xml(&log)?;
    let entry = doc
        .descendants()
        .find(|node| node.has_tag_name("logentry"))
        .ok_or_else(|| invalid("The selected SVN revision is unavailable."))?;
    let mut files = Vec::new();
    let mut other = Vec::new();
    let mut limited = false;
    let mut size = 0;
    const MAX_FILES: usize = 100;
    const MAX_BYTES: usize = 12 * 1024 * 1024;
    for (index, node) in entry
        .descendants()
        .filter(|node| node.has_tag_name("path"))
        .enumerate()
    {
        cancellation.check()?;
        let (Some(path), Some(action)) = (node.text(), node.attribute("action")) else {
            continue;
        };
        let relative_path = path.trim_start_matches('/');
        if focus_path.is_some_and(|focus| focus != relative_path) {
            continue;
        }
        if (focus_path.is_none() && index >= MAX_FILES) || size >= MAX_BYTES {
            limited = true;
            other.push(path);
            continue;
        }
        let directory = node.attribute("kind") == Some("dir");
        let mut review = match revision_file(copy, path, rev, action, directory, invoke) {
            Ok(review) => review,
            Err(error) if matches!(error.kind, "output_limit" | "binary") => {
                other.push(path);
                continue;
            }
            Err(error) => return Err(error),
        };
        let mut included = false;
        for (part, mut file) in [review["file"].take(), review["details"].take()]
            .into_iter()
            .enumerate()
        {
            if file.is_null() {
                continue;
            }
            let bytes = ["base", "head"]
                .iter()
                .filter_map(|key| file[*key].as_str())
                .map(str::len)
                .sum::<usize>();
            if size + bytes > MAX_BYTES {
                limited = true;
                continue;
            }
            file["path"] = relative_path.into();
            file["sort_order"] = (2 * index + part).into();
            size += bytes;
            included = true;
            files.push(file);
        }
        if !included {
            other.push(path);
        }
    }
    Ok(json!({
        "revision":rev,
        "author":text(entry,"author").unwrap_or_default(),
        "date":text(entry,"date").unwrap_or_default(),
        "message":text(entry,"msg").unwrap_or_default(),
        "files":files,
        "other":other,
        "limited":limited,
        "stats":stats,
    }))
}

fn patch_has_properties(patch: &str) -> bool {
    patch
        .lines()
        .any(|line| line.starts_with("Property changes on: "))
}

fn diff_reason(patch: &str, directory: bool) -> &'static str {
    if directory {
        "directory"
    } else if patch.contains("Cannot display: file marked as a binary type") {
        "binary"
    } else {
        "empty"
    }
}

fn read_properties(
    invoke: &impl Fn(Vec<OsString>, Vec<OsString>) -> Result<String>,
    target: OsString,
    revision: Option<&str>,
) -> Result<BTreeMap<String, String>> {
    let mut args = flags(&["proplist", "--xml", "--verbose", "--depth", "empty"]);
    if let Some(revision) = revision {
        args.extend(flags(&["-r", revision]));
    }
    let output = invoke(args, vec![target])?;
    let document = xml(&output)?;
    Ok(document
        .descendants()
        .filter(|node| node.has_tag_name("property"))
        .filter_map(|node| {
            let name = node.attribute("name")?.to_owned();
            let value = node.text().unwrap_or_default();
            let value = if node.attribute("encoding") == Some("base64") {
                format!("[base64] {}", value.split_whitespace().collect::<String>())
            } else {
                value.to_owned()
            };
            Some((name, value))
        })
        .collect())
}

fn property_diff(
    path: &str,
    from: String,
    to: String,
    load: impl Fn(bool) -> Result<BTreeMap<String, String>>,
) -> Result<Value> {
    let base = load(false)?;
    let head = load(true)?;
    let changed = |current: &BTreeMap<String, String>, other: &BTreeMap<String, String>| {
        current
            .iter()
            .filter(|(name, value)| other.get(*name) != Some(*value))
            .map(|(name, value)| format!("{} = {}\n", name, value.replace('\n', "\n    ")))
            .collect::<String>()
    };
    let mut pair = diff_pair(path, changed(&base, &head), changed(&head, &base), from, to);
    if !pair.is_null() {
        pair["kind"] = "properties".into();
    }
    Ok(pair)
}

fn history_file_url(copy: &WorkingCopy, path: &str) -> Result<String> {
    if !path.starts_with('/')
        || path == "/"
        || path
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || matches!(part, "." | "..") || part.contains('\0'))
    {
        return Err(invalid("Select a repository-relative SVN file."));
    }
    let address = encode_url_path(copy.repository_url.trim_end_matches('/').to_owned(), path);
    repository_target(copy, &address)?;
    Ok(address)
}

fn encode_url_path(mut address: String, path: &str) -> String {
    use std::fmt::Write as _;
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            address.push(char::from(byte));
        } else {
            let _ = write!(address, "%{byte:02X}");
        }
    }
    address
}

fn url_bytes(address: &str) -> impl Iterator<Item = u8> + '_ {
    let mut bytes = address.as_bytes();
    std::iter::from_fn(move || {
        let (&byte, remaining) = bytes.split_first()?;
        if byte == b'%'
            && let [high, low, tail @ ..] = remaining
            && let Some(high) = char::from(*high).to_digit(16)
            && let Some(low) = char::from(*low).to_digit(16)
        {
            bytes = tail;
            return Some((high * 16 + low) as u8);
        }
        bytes = remaining;
        Some(byte)
    })
}

fn format_history(output: &str) -> Result<Value> {
    let document = xml(output)?;
    let mut revisions = Vec::new();
    for entry in document
        .descendants()
        .filter(|node| node.has_tag_name("logentry"))
    {
        let Some(revision) = entry
            .attribute("revision")
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        let author = text(entry, "author").unwrap_or_default();
        let date = text(entry, "date").unwrap_or_default();
        let message = text(entry, "msg").unwrap_or_default();
        let paths = entry.descendants().filter(|node| node.has_tag_name("path")).map(|node| {
            let path = node.text().unwrap_or_default();
            let action = node.attribute("action").unwrap_or_default();
            json!({"path":path,"action":action,"kind":node.attribute("kind").unwrap_or_default()})
        }).collect::<Vec<_>>();
        revisions.push(json!({"revision":revision,"author":author,"date":date,"message":message,"paths":paths}));
    }
    let next_before = (revisions.len() == 50)
        .then(|| revisions.last())
        .flatten()
        .and_then(|revision| revision["revision"].as_u64())
        .filter(|revision| *revision > 1)
        .map(|revision| revision - 1);
    Ok(json!({"revisions":revisions,"next_before":next_before}))
}

pub(super) fn incoming_tree_add<'a>(document: &'a Document<'_>) -> Option<roxmltree::Node<'a, 'a>> {
    let tree = document
        .descendants()
        .find(|node| node.has_tag_name("tree-conflict"))?;
    if tree.attribute("operation") != Some("update")
        || tree.attribute("reason") != Some("add")
        || tree.attribute("action") != Some("add")
        || tree.attribute("kind") != Some("file")
        || tree
            .children()
            .any(|node| node.attribute("side") == Some("source-left"))
    {
        return None;
    }
    tree.children().find(|node| {
        node.has_tag_name("version")
            && node.attribute("side") == Some("source-right")
            && node.attribute("kind") == Some("file")
    })
}

fn conflict(
    copy: &WorkingCopy,
    path: &str,
    auth: &Auth,
    cancellation: &Cancellation,
) -> Result<Value> {
    let target = file_target(&copy.root, path)?;
    let output = command(
        Some(&copy.root),
        flags(&["info", "--xml"]),
        vec![peg(&target)],
        &Auth::default(),
        None,
        cancellation,
    )?;
    let document = xml(&output)?;
    let mut views = serde_json::Map::new();
    for (xml_tag, view) in CONFLICT_ARTIFACTS {
        if let Some(raw) = document
            .descendants()
            .find(|node| node.has_tag_name(xml_tag))
            .and_then(|node| node.text())
        {
            let raw = Path::new(raw);
            let absolute = if raw.is_absolute() {
                raw.to_path_buf()
            } else {
                copy.root.join(raw)
            };
            if let Ok(canonical) = fs::canonicalize(&absolute)
                && canonical.starts_with(&copy.root)
                && canonical
                    .metadata()
                    .is_ok_and(|metadata| metadata.is_file() && metadata.len() < 4 * 1024 * 1024)
                && let Ok(content) = fs::read_to_string(canonical)
            {
                views.insert(view.into(), Value::String(content));
            }
        }
    }
    if let Some(content) = read_small_text(&target) {
        views.insert("working".into(), Value::String(content));
    }
    if let Some(comparison) = views
        .get("property_reject")
        .and_then(Value::as_str)
        .and_then(property_conflict_values)
    {
        for (view, value) in comparison {
            views.insert(view.into(), Value::String(value));
        }
    }
    let property_conflict = document
        .descendants()
        .any(|node| node.has_tag_name("conflict") && node.attribute("type") == Some("property"));
    let text_conflict = document
        .descendants()
        .any(|node| node.has_tag_name("conflict") && node.attribute("type") == Some("text"));
    let mut incoming_revision = None;
    let mut tree_preview_error = None;
    if !text_conflict
        && !property_conflict
        && views.contains_key("working")
        && let Some(incoming) = incoming_tree_add(&document)
        && let (Some(path), Some(rev)) = (
            incoming.attribute("path-in-repos"),
            incoming.attribute("revision"),
        )
    {
        let rev = revision(rev)?;
        let address = history_file_url(copy, &format!("/{path}"))?;
        match command(
            Some(&copy.root),
            flags(&["cat", "-r", rev]),
            vec![format!("{address}@{rev}").into()],
            auth,
            Some(&copy.repository_url),
            cancellation,
        ) {
            Ok(content) if content.len() <= MAX_DIFF_TEXT && !content.contains('\0') => {
                views.insert("tree_incoming".into(), Value::String(content));
                incoming_revision = rev.parse::<u64>().ok();
            }
            Ok(_) => {}
            Err(error) if error.kind == "cancelled" => return Err(error),
            Err(error) => {
                tree_preview_error = Some(json!({"kind":error.kind,"message":error.message}));
            }
        }
    }
    let tree = document
        .descendants()
        .find(|node| node.has_tag_name("tree-conflict"))
        .map(|node| {
            let version = |side| {
                node.children()
                    .find(|entry| {
                        entry.has_tag_name("version") && entry.attribute("side") == Some(side)
                    })
                    .map(|entry| {
                        json!({
                            "path":entry.attribute("path-in-repos"),
                            "revision":entry.attribute("revision"),
                            "kind":entry.attribute("kind"),
                        })
                    })
            };
            json!({
                "reason":node.attribute("reason"),
                "action":node.attribute("action"),
                "operation":node.attribute("operation"),
                "base":version("source-left"),
                "incoming":version("source-right"),
            })
        });
    let details = command(
        Some(&copy.root),
        flags(&["info"]),
        vec![peg(&target)],
        &Auth::default(),
        None,
        cancellation,
    )?;
    Ok(json!({
        "path": path,
        "text": details,
        "views": views,
        "text_conflict": text_conflict,
        "property_conflict": property_conflict,
        "tree": tree,
        "tree_incoming_revision": incoming_revision,
        "tree_preview_error": tree_preview_error,
    }))
}

// SVN writes property conflicts as a text reject file. The delimiters include
// full labels, and the value immediately before a delimiter may end mid-line.
fn property_conflict_values(reject: &str) -> Option<[(&'static str, String); 3]> {
    use std::fmt::Write as _;

    const LOCAL: &str = "<<<<<<< (local property value)\n";
    const BASE: &str = "||||||| (incoming 'changed from' value)\n";
    const INCOMING: &str = ">>>>>>> (incoming 'changed to' value)";
    let mut rest = reject;
    let mut views = [
        ("property_base", String::new()),
        ("property_mine", String::new()),
        ("property_incoming", String::new()),
    ];
    while !rest.trim().is_empty() {
        let (header, tail) = rest.split_once(LOCAL)?;
        let (_, header) = header.rsplit_once("property '")?;
        let (name, _) = header.split_once("'\n")?;
        let (mine, tail) = tail.split_once(BASE)?;
        let (base, tail) = tail.split_once("=======\n")?;
        let (incoming, after) = tail.split_once(INCOMING)?;
        for ((_, view), value) in views.iter_mut().zip([base, mine, incoming]) {
            let _ = writeln!(view, "{name} = {value}");
        }
        rest = after;
    }
    (!views[0].1.is_empty()).then_some(views)
}
