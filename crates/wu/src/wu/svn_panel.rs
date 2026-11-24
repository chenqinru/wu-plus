mod branch_picker;
mod changes_view;
mod commit_message;
mod conflicts;
mod editor_support;
mod form;
mod history;
mod panel_ui;
pub(super) mod preview;
mod rpc;

use editor::Editor;
use extension_host::ExtensionStore;
use futures::{FutureExt as _, future::Shared};
use git::status::{GitSummary, TrackedSummary};
use gpui::{
    Action, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    PromptLevel, ScrollHandle, Subscription, Task, TaskExt as _, WeakEntity, Window, actions, px,
};
use i18n::{n, t};
use notifications::status_toast::StatusToast;
use project::project_settings::ProjectSettings;
use serde::Deserialize;
use serde_json::{Value, json};
use settings::{Settings, SettingsStore};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use ui::{Checkbox, Tooltip, prelude::*};
use util::ResultExt as _;
use workspace::{
    OpenOptions, OpenVisible, Panel, Workspace,
    dock::{DockPosition, PanelEvent},
};

actions!(svn, [TogglePanel]);

#[derive(Clone, Deserialize)]
struct Copy {
    root: PathBuf,
    url: String,
    repository_url: String,
    revision: String,
}

#[derive(Clone, Deserialize)]
struct Change {
    path: String,
    #[serde(default)]
    is_dir: bool,
    item: String,
    props: String,
    tree_conflicted: bool,
    file_external: bool,
    copied: bool,
    switched: bool,
    locked: bool,
    lock: Option<FileLock>,
    changelist: Option<String>,
    moved_from: Option<String>,
    moved_to: Option<String>,
    remote_item: Option<String>,
    remote_props: Option<String>,
    #[serde(default)]
    diff_stat: Option<DiffStat>,
}

#[derive(Clone, Deserialize)]
struct FileLock {
    owner: Option<String>,
    comment: Option<String>,
    held_here: bool,
}

#[derive(Clone, Copy, Default, Deserialize)]
struct DiffStat {
    added: usize,
    deleted: usize,
}

impl Change {
    fn can_unlock(&self) -> bool {
        self.locked && self.lock.as_ref().is_none_or(|lock| lock.held_here)
    }

    fn conflicted(&self) -> bool {
        self.tree_conflicted || self.item == "conflicted" || self.props == "conflicted"
    }

    fn can_view_diff(&self) -> bool {
        if self.file_external {
            return false;
        }
        self.conflicted()
            || self.committable()
            || self.item == "missing"
            || self.item == "unversioned" && !self.is_dir
    }

    fn label(&self) -> &'static str {
        if self.conflicted() {
            return "C";
        }
        match self.item.as_str() {
            "unversioned" => "?",
            "added" => "A",
            "deleted" => "D",
            "replaced" => "R",
            "missing" => "!",
            "ignored" => "I",
            "external" => "X",
            "modified" => "M",
            "normal" if self.props == "modified" => "P",
            _ => "·",
        }
    }

    fn committable(&self) -> bool {
        !self.conflicted()
            && !self.file_external
            && (matches!(
                self.item.as_str(),
                "added" | "deleted" | "modified" | "replaced"
            ) || self.item == "normal" && self.props == "modified")
    }
}

#[derive(Deserialize)]
struct Discovery {
    copies: Vec<Copy>,
    warnings: Vec<String>,
}

#[derive(Deserialize)]
struct Status {
    copy: Copy,
    entries: Vec<Change>,
}

struct Repo {
    copy: Copy,
    entries: Vec<Change>,
    error: Option<String>,
}

impl Repo {
    fn change_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.can_view_diff() || entry.item == "unversioned")
            .count()
    }
}

fn project_statuses(repos: &[Repo]) -> BTreeMap<PathBuf, GitSummary> {
    let roots = repos
        .iter()
        .map(|repo| repo.copy.root.as_path())
        .collect::<BTreeSet<_>>();
    let mut statuses = BTreeMap::<PathBuf, GitSummary>::new();
    for repo in repos {
        let root = &repo.copy.root;
        for change in &repo.entries {
            let status = match change.label() {
                "C" => GitSummary::CONFLICT,
                "?" => GitSummary::UNTRACKED,
                "A" => GitSummary {
                    index: TrackedSummary::ADDED,
                    count: 1,
                    ..GitSummary::UNCHANGED
                },
                "D" | "!" => GitSummary {
                    worktree: TrackedSummary::DELETED,
                    count: 1,
                    ..GitSummary::UNCHANGED
                },
                "M" | "R" | "P" => GitSummary {
                    worktree: TrackedSummary::MODIFIED,
                    count: 1,
                    ..GitSummary::UNCHANGED
                },
                _ => continue,
            };
            let path = if change.path == "." {
                root.clone()
            } else {
                root.join(&change.path)
            };
            // An outer checkout may report a nested checkout as unversioned.
            if path != *root
                && path
                    .ancestors()
                    .take_while(|path| *path != root)
                    .any(|path| roots.contains(path))
            {
                continue;
            }
            for path in path.ancestors().take_while(|path| path.starts_with(root)) {
                *statuses.entry(path.to_path_buf()).or_default() += status;
            }
        }
    }
    statuses
}

#[derive(Clone, PartialEq, Eq)]
enum Provider {
    Bundled(PathBuf),
    Development(PathBuf),
}

impl Provider {
    fn executable(&self) -> Result<PathBuf, rpc::Error> {
        match self {
            Self::Bundled(executable) => Ok(executable.clone()),
            Self::Development(directory) => rpc::development_executable(directory),
        }
    }
}

const EXTENSION_ID: &str = "wu-plus-subversion";

fn has_svn_working_copy(roots: &[PathBuf]) -> bool {
    let mut directories = Vec::new();
    for root in roots {
        let root = root.canonicalize().unwrap_or_else(|_| root.clone());
        if root
            .ancestors()
            .any(|directory| directory.join(".svn").is_dir())
        {
            return true;
        }
        if root.is_dir() {
            directories.push(root);
        }
    }

    let mut visited = BTreeSet::new();
    while let Some(directory) = directories.pop() {
        if !visited.insert(directory.clone()) {
            continue;
        }
        // Let the provider handle very large scans and report its discovery limit.
        if visited.len() > 75_000 || directory.join(".svn").is_dir() {
            return true;
        }
        if let Ok(entries) = directory.read_dir() {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && !matches!(
                        entry.file_name().to_str(),
                        Some(".git" | ".hg" | ".svn" | "node_modules")
                    )
                {
                    directories.push(entry.path());
                }
            }
        }
    }
    false
}

type Connection = Shared<Task<Result<Arc<rpc::Client>, rpc::Error>>>;

#[derive(Clone, serde::Serialize)]
struct Credentials {
    username: String,
    password: Option<String>,
    remember: bool,
}

#[derive(Clone, Copy)]
enum Output {
    Text,
    Diff,
    RevisionReview { revision: u64 },
    WorktreeDiff,
    Blame,
    Change,
    Commit,
    Checkout,
    Conflict,
    RemoteStatus,
}

enum PreviewRequest {
    Diff(u64),
    Revision {
        id: Option<u64>,
        root: PathBuf,
        revision: u64,
        target: Option<String>,
        focused: bool,
        background: bool,
    },
    Worktree {
        id: Option<u64>,
        revision: u64,
        target: Option<String>,
        focused: bool,
        background: bool,
    },
    File(Task<()>),
}

pub(super) struct SvnPanel {
    workspace: WeakEntity<Workspace>,
    connecting: Option<Connection>,
    project: Entity<project::Project>,
    focus: FocusHandle,
    position: DockPosition,
    scroll: ScrollHandle,
    repositories_scroll: ScrollHandle,
    history: history::History,
    history_requests: history::Requests,
    commit_editor: Entity<Editor>,
    commit_drafts: BTreeMap<PathBuf, String>,
    commit_generation: Option<commit_message::Generation>,
    commit_editor_expanded: bool,
    provider: Option<Provider>,
    client: Option<Arc<rpc::Client>>,
    repos: Vec<Repo>,
    selected_root: Option<PathBuf>,
    checkout_root: Option<PathBuf>,
    selected_paths: BTreeSet<String>,
    repository_selections: BTreeMap<PathBuf, BTreeSet<String>>,
    active_path: Option<String>,
    credentials: BTreeMap<PathBuf, Credentials>,
    enabled: bool,
    active: bool,
    busy: bool,
    scanning: bool,
    revision: u64,
    operation_id: Option<u64>,
    cancellation_requested: bool,
    view_options: changes_view::ViewOptions,
    save_view_options: Option<Task<()>>,
    collapsed_groups: BTreeSet<changes_view::Section>,
    collapsed_directories: BTreeSet<(changes_view::Section, String)>,
    active_directory: Option<(changes_view::Section, String)>,
    repositories_collapsed: bool,
    preview_generation: u64,
    preview_request: Option<PreviewRequest>,
    preview_tab: Option<(WeakEntity<workspace::Pane>, gpui::EntityId)>,
    pending_refresh: Option<bool>,
    scan_error: Option<String>,
    last_output: Option<String>,
    error_kind: Option<String>,
    certificate_target: Option<String>,
    form: Option<form::Form>,
    _events: Vec<Subscription>,
    refresh: Option<Task<()>>,
    operation: Option<Task<()>>,
    deferred_refresh: Option<Task<()>>,
}

impl SvnPanel {
    pub(super) fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: gpui::AsyncWindowContext,
    ) -> Task<anyhow::Result<Entity<Self>>> {
        let workspace_handle = workspace.clone();
        let result = workspace.update_in(&mut cx, |workspace, window, cx| {
            let project = workspace.project().clone();
            let store = ExtensionStore::try_global(cx);
            cx.new(|cx| Self::new(workspace_handle, project, store, window, cx))
        });
        Task::ready(result)
    }

    fn new(
        workspace: WeakEntity<Workspace>,
        project: Entity<project::Project>,
        store: Option<Entity<ExtensionStore>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let commit_editor = cx.new(|cx| {
            let mut editor = Editor::auto_height(6, 6, window, cx);
            editor.set_soft_wrap();
            editor.set_show_gutter(false, cx);
            editor.set_show_wrap_guides(false, cx);
            editor.set_show_indent_guides(false, cx);
            editor.set_placeholder_text(t!("Commit Message").as_ref(), window, cx);
            editor
        });
        let commit_events = cx.observe(&commit_editor, |_, _, cx| cx.notify());
        let settings_store = store.clone();
        let mut subscriptions = vec![
            commit_events,
            cx.observe_global_in::<SettingsStore>(window, move |this, window, cx| {
                let provider = this.resolve_provider(settings_store.as_ref(), cx);
                this.sync_provider(provider, window, cx);
            }),
            cx.subscribe_in(&project, window, |this, _, event, window, cx| match event {
                project::Event::WorktreeAdded(_)
                | project::Event::WorktreeRemoved(_)
                | project::Event::WorktreePathsChanged { .. } => {
                    this.schedule_refresh(true, window, cx)
                }
                project::Event::WorktreeUpdatedEntries(id, changes) => {
                    let changed =
                        this.project
                            .read(cx)
                            .worktree_for_id(*id, cx)
                            .is_some_and(|tree| {
                                let worktree = tree.read(cx);
                                changes.iter().any(|(path, _, change)| {
                                    if *change == project::PathChange::Loaded {
                                        return false;
                                    }
                                    let relative_path = path.as_std_path();
                                    if relative_path
                                        .components()
                                        .any(|part| part.as_os_str() == ".svn")
                                    {
                                        return false;
                                    }
                                    if this.repos.is_empty() {
                                        return worktree
                                            .entry_for_path(path)
                                            .is_some_and(|entry| entry.is_dir());
                                    }
                                    let path = worktree.abs_path().join(relative_path);
                                    this.repos.iter().any(|repo| {
                                        path.starts_with(&repo.copy.root)
                                            || repo.copy.root.starts_with(&path)
                                    })
                                })
                            });
                    if changed {
                        this.schedule_refresh(false, window, cx);
                    }
                }
                _ => {}
            }),
        ];
        if let Some(store) = &store {
            subscriptions.push(
                cx.subscribe_in(store, window, |this, store, event, window, cx| {
                    if matches!(
                        event,
                        extension_host::Event::ExtensionsUpdated
                            | extension_host::Event::ExtensionInstalled(_)
                            | extension_host::Event::ExtensionUninstalled(_)
                    ) {
                        let provider = this.resolve_provider(Some(&store), cx);
                        this.sync_provider(provider, window, cx);
                    }
                }),
            );
        }
        if let Some(workspace) = workspace.upgrade() {
            subscriptions.push(cx.subscribe_in(
                &workspace,
                window,
                |this, _, event, window, cx| {
                    if matches!(event, workspace::Event::ActiveItemChanged) {
                        this.bind_active_editor(window, cx);
                    }
                },
            ));
        }
        let mut this = Self {
            workspace,
            connecting: None,
            project,
            focus: cx.focus_handle(),
            position: DockPosition::Left,
            scroll: ScrollHandle::new(),
            repositories_scroll: ScrollHandle::new(),
            history: history::History::default(),
            history_requests: history::Requests::default(),
            commit_editor,
            commit_drafts: BTreeMap::new(),
            commit_generation: None,
            commit_editor_expanded: false,
            provider: None,
            client: None,
            repos: Vec::new(),
            selected_root: None,
            checkout_root: None,
            selected_paths: BTreeSet::new(),
            repository_selections: BTreeMap::new(),
            active_path: None,
            credentials: BTreeMap::new(),
            enabled: false,
            active: false,
            busy: false,
            scanning: false,
            revision: 0,
            operation_id: None,
            cancellation_requested: false,
            view_options: changes_view::ViewOptions::load(cx),
            save_view_options: None,
            collapsed_groups: BTreeSet::new(),
            collapsed_directories: BTreeSet::new(),
            active_directory: None,
            repositories_collapsed: false,
            preview_generation: 0,
            preview_request: None,
            preview_tab: None,
            pending_refresh: None,
            scan_error: None,
            last_output: None,
            error_kind: None,
            certificate_target: None,
            form: None,
            _events: subscriptions,
            refresh: None,
            operation: None,
            deferred_refresh: None,
        };
        let provider = this.resolve_provider(store.as_ref(), cx);
        this.sync_provider(provider, window, cx);
        this
    }

    fn resolve_provider(
        &self,
        store: Option<&Entity<ExtensionStore>>,
        cx: &App,
    ) -> Option<Provider> {
        if !self.project.read(cx).is_local() || !ProjectSettings::get_global(cx).svn.enabled {
            return None;
        }
        if let Ok(executable) = cx.path_for_auxiliary_executable(rpc::EXECUTABLE_NAME)
            && executable.is_file()
        {
            return Some(Provider::Bundled(executable));
        }
        let store = store?.read(cx);
        store
            .dev_extensions()
            .any(|extension| extension.id.as_ref() == EXTENSION_ID)
            .then(|| Provider::Development(store.installed_dir.join(EXTENSION_ID)))
    }

    fn sync_provider(
        &mut self,
        provider: Option<Provider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if provider == self.provider {
            return;
        }
        if let (Some(client), Some(id)) = (&self.client, self.operation_id.take()) {
            client.cancel(id);
        }
        self.clear_preview(window, cx);
        if provider.is_none() {
            preview::close_worktree_diff(&self.workspace, window, cx);
        }
        self.cancel_commit_generation(cx);
        self.pending_refresh = None;
        self.revision += 1;
        self.enabled = provider.is_some();
        self.provider = provider;
        self.client = None;
        self.connecting = None;
        self.refresh = None;
        self.operation = None;
        self.deferred_refresh = None;
        self.busy = false;
        self.scanning = false;
        self.form = None;
        self.credentials.clear();
        self.history_requests.clear();
        self.checkout_root = None;
        if !self.repos.is_empty() {
            self.repos.clear();
            self.publish_project_statuses(BTreeMap::new(), cx);
        }
        self.set_current(None, window, cx);
        self.repository_selections.clear();
        self.scan_error = None;
        self.error_kind = None;
        self.certificate_target = None;
        self.refresh(true, window, cx);
        cx.notify();
        let panel = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            if let Some(panel) = panel.upgrade() {
                panel.update(cx, |this, cx| {
                    this.detach_editor_history(window, cx);
                    this.bind_active_editor(window, cx);
                });
            }
        });
    }

    fn connection(&mut self, cx: &App) -> Option<Connection> {
        if let Some(client) = &self.client {
            return Some(Task::ready(Ok(client.clone())).shared());
        }
        let provider = self.provider.clone()?;
        Some(
            self.connecting
                .get_or_insert_with(|| {
                    cx.background_spawn(async move {
                        rpc::Client::connect(&provider.executable()?).await
                    })
                    .shared()
                })
                .clone(),
        )
    }

    fn schedule_refresh(&mut self, discover: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        let discover = discover || self.repos.is_empty() || self.pending_refresh == Some(true);
        self.pending_refresh = Some(discover);
        if self.busy || self.scanning {
            return;
        }
        self.deferred_refresh = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(450))
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(discover) = this.pending_refresh {
                    this.refresh(discover, window, cx);
                }
            });
        }));
    }

    fn refresh(&mut self, discover: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.scanning {
            self.pending_refresh = Some(discover || self.pending_refresh == Some(true));
            return;
        }
        if self.provider.is_none() {
            return;
        }
        let current_roots = self
            .project
            .read(cx)
            .visible_worktrees(cx)
            .map(|tree| tree.read(cx).abs_path().to_path_buf())
            .collect::<Vec<_>>();
        let existing = self
            .repos
            .iter()
            .map(|repo| repo.copy.clone())
            .collect::<Vec<_>>();
        let discover = discover || existing.is_empty() || self.pending_refresh == Some(true);
        self.pending_refresh = None;
        self.deferred_refresh = None;
        let ignored = self.view_options.show_ignored;
        let needs_probe = !self.active && self.client.is_none() && self.connecting.is_none();
        let probe = needs_probe.then(|| {
            let roots = current_roots.clone();
            cx.background_spawn(async move { has_svn_working_copy(&roots) })
        });
        self.revision += 1;
        let revision = self.revision;
        self.scanning = true;
        self.scan_error = None;
        self.refresh = Some(cx.spawn_in(window, async move |this, cx| {
            if let Some(probe) = probe
                && !probe.await
            {
                this.update_in(cx, |this, window, cx| {
                    if revision == this.revision {
                        this.scanning = false;
                        if this.active || this.pending_refresh.is_some() {
                            this.schedule_refresh(true, window, cx);
                        }
                        cx.notify();
                    }
                })
                .log_err();
                return;
            }
            let Ok(Some(connection)) = this.update(cx, |this, cx| {
                if revision != this.revision {
                    return None;
                }
                this.connection(cx)
            }) else {
                return;
            };
            let scan = cx.background_spawn(async move {
                let client = connection.await?;
                let (copies, warnings) = if discover {
                    let reply = client
                        .request(client.next_id(), "discover", json!({"roots":current_roots}))
                        .await?;
                    let result =
                        serde_json::from_value::<Discovery>(reply).map_err(decode_error)?;
                    (result.copies, result.warnings)
                } else {
                    (existing, Vec::new())
                };
                let mut repos = Vec::with_capacity(copies.len());
                for copy in copies {
                    let result = client
                        .request(
                            client.next_id(),
                            "status",
                            json!({"root":copy.root,"ignored":ignored,"diff_stats":true}),
                        )
                        .await
                        .and_then(|value| {
                            serde_json::from_value::<Status>(value).map_err(decode_error)
                        });
                    repos.push(match result {
                        Ok(Status { copy, entries }) => Repo {
                            copy,
                            entries,
                            error: None,
                        },
                        Err(error) => Repo {
                            copy,
                            entries: Vec::new(),
                            error: Some(error.message),
                        },
                    });
                }
                let statuses = project_statuses(&repos);
                Ok::<_, rpc::Error>((client, repos, warnings, statuses))
            });
            let result = scan.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if revision != this.revision {
                    return;
                }
                this.scanning = false;
                this.connecting = None;
                match result {
                    Ok((client, repos, warnings, statuses)) => {
                        this.client = (!repos.is_empty() || this.active).then_some(client);
                        if discover {
                            let previous = this
                                .repos
                                .iter()
                                .map(|repo| &repo.copy.root)
                                .collect::<BTreeSet<_>>();
                            let current = repos
                                .iter()
                                .map(|repo| &repo.copy.root)
                                .collect::<BTreeSet<_>>();
                            let changed_roots = previous
                                .symmetric_difference(&current)
                                .map(|root| (**root).clone())
                                .collect();
                            this.recheck_editor_locations(&changed_roots, window, cx);
                        }
                        this.repos = repos;
                        this.publish_project_statuses(statuses, cx);
                        let contains =
                            |path: &PathBuf| this.repos.iter().any(|repo| &repo.copy.root == path);
                        let current = if this.checkout_root.as_ref().is_some_and(contains) {
                            this.checkout_root.take()
                        } else {
                            this.selected_root
                                .as_ref()
                                .filter(|root| contains(root))
                                .cloned()
                                .or_else(|| this.repos.first().map(|repo| repo.copy.root.clone()))
                        };
                        if current != this.selected_root {
                            this.set_current(current, window, cx);
                            if this.history.visible && this.selected_root.is_some() {
                                this.load_history(false, window, cx);
                            }
                        }
                        this.prune_selection();
                        if this.pending_refresh.is_none()
                            && this.preview_request.is_none()
                            && preview::worktree_is_active(&this.workspace, cx)
                        {
                            let target = this.repo().and_then(|repo| {
                                repo.entries.iter().find_map(|entry| {
                                    (this.active_path.as_ref() == Some(&entry.path)
                                        && entry.can_view_diff()
                                        && !entry.conflicted())
                                    .then(|| entry.moved_to.as_ref().unwrap_or(&entry.path).clone())
                                })
                            });
                            this.show_worktree_diff(target, window, cx);
                        }
                        if let Some(first) = warnings.first() {
                            let output = warnings.join("\n");
                            this.last_output = Some(output.clone());
                            this.show_output_status(first.clone(), output, true, cx);
                        }
                    }
                    Err(error) => {
                        this.client = None;
                        this.history_requests.clear();
                        this.scan_error = Some(match error.kind.as_str() {
                            "svn_unavailable" | "svn_version" => {
                                t!("Install Subversion 1.14 or newer (brew install subversion).")
                                    .to_string()
                            }
                            _ => error.message,
                        });
                    }
                }
                if let Some(discover) = this.pending_refresh {
                    this.schedule_refresh(discover, window, cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn publish_project_statuses(&self, statuses: BTreeMap<PathBuf, GitSummary>, cx: &mut App) {
        if let Some(panel) = self
            .workspace
            .read_with(cx, |workspace, cx| {
                workspace.panel::<project_panel::ProjectPanel>(cx)
            })
            .ok()
            .flatten()
        {
            panel.update(cx, |panel, cx| panel.set_external_statuses(statuses, cx));
        }
    }

    fn prune_selection(&mut self) {
        if let Some(repo) = self
            .repos
            .iter()
            .find(|repo| Some(&repo.copy.root) == self.selected_root.as_ref())
        {
            self.selected_paths
                .retain(|path| repo.entries.iter().any(|entry| &entry.path == path));
            if self
                .active_path
                .as_ref()
                .is_some_and(|path| !repo.entries.iter().any(|entry| &entry.path == path))
            {
                self.active_path = None;
            }
        }
    }

    fn set_current(&mut self, root: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_root == root {
            return;
        }
        self.clear_preview(window, cx);
        self.cancel_commit_generation(cx);
        if let Some(previous) = &self.selected_root {
            self.repository_selections
                .insert(previous.clone(), std::mem::take(&mut self.selected_paths));
            let message = self.commit_editor.read(cx).text(cx);
            if message.is_empty() {
                self.commit_drafts.remove(previous);
            } else {
                self.commit_drafts.insert(previous.clone(), message);
            }
        }
        self.selected_root = root;
        self.reveal_selected_repository();
        let draft = self
            .selected_root
            .as_ref()
            .and_then(|root| self.commit_drafts.get(root))
            .cloned()
            .unwrap_or_default();
        self.commit_editor
            .update(cx, |editor, cx| editor.set_text(draft, window, cx));
        self.active_path = None;
        self.active_directory = None;
        self.collapsed_directories.clear();
        self.selected_paths = self
            .selected_root
            .as_ref()
            .and_then(|root| self.repository_selections.remove(root))
            .unwrap_or_default();
        self.prune_selection();
        self.scroll.set_offset(gpui::point(px(0.), px(0.)));
        self.history.clear();
        self.last_output = None;
        self.form = None;
    }

    fn switch_repository(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy
            || self.selected_root.as_ref() == Some(&root)
            || !self.repos.iter().any(|repo| repo.copy.root == root)
        {
            return;
        }
        let showing_diff = preview::worktree_is_active(&self.workspace, cx);
        self.checkout_root = None;
        self.set_current(Some(root), window, cx);
        self.error_kind = None;
        self.certificate_target = None;
        if self.history.visible {
            self.load_history(false, window, cx);
        } else if showing_diff {
            self.show_worktree_diff(None, window, cx);
        }
        cx.notify();
    }

    fn cancel_preview_request(&mut self) {
        self.preview_generation += 1;
        match self.preview_request.take() {
            Some(
                PreviewRequest::Diff(id)
                | PreviewRequest::Worktree { id: Some(id), .. }
                | PreviewRequest::Revision { id: Some(id), .. },
            ) => {
                if let Some(client) = &self.client {
                    client.cancel(id);
                }
            }
            Some(PreviewRequest::File(task)) => drop(task),
            Some(
                PreviewRequest::Worktree { id: None, .. }
                | PreviewRequest::Revision { id: None, .. },
            )
            | None => {}
        }
    }

    fn clear_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_preview_request();
        if let Some((pane, id)) = self.preview_tab.take() {
            let _ = pane.update(cx, |pane, cx| {
                if pane.is_active_preview_item(id)
                    && pane.preview_item().is_some_and(|item| !item.is_dirty(cx))
                {
                    pane.close_item_by_id(id, workspace::SaveIntent::Close, window, cx)
                        .detach_and_log_err(cx);
                }
            });
        }
    }

    fn request_preview(
        &mut self,
        client: Arc<rpc::Client>,
        root: Option<PathBuf>,
        mut params: Value,
        output: Output,
        certificate_target: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(arguments) = params.as_object_mut() else {
            return;
        };
        self.cancel_preview_request();
        let generation = self.preview_generation;
        let id = client.next_id();
        let background = arguments
            .remove("background")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let close_form = arguments
            .remove("close_form")
            .and_then(|value| value.as_bool())
            == Some(true);
        let focus_path = arguments
            .get("focus_path")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.preview_request = Some(match output {
            Output::WorktreeDiff => PreviewRequest::Worktree {
                id: Some(id),
                revision: self.revision,
                focused: focus_path.is_some(),
                target: focus_path,
                background,
            },
            Output::RevisionReview { revision } => PreviewRequest::Revision {
                id: Some(id),
                root: root.clone().expect("revision review has a working copy"),
                revision,
                target: arguments
                    .remove("path")
                    .and_then(|path| path.as_str().map(str::to_owned)),
                focused: focus_path.is_some(),
                background,
            },
            _ => PreviewRequest::Diff(id),
        });
        let form_id = self.form.as_ref().and_then(form::Form::first_field_id);
        let task = cx.background_spawn(async move { client.request(id, "read", params).await });
        cx.spawn_in(window, async move |panel, cx| {
            let result = task.await;
            let _ = panel.update_in(cx, |this, window, cx| {
                if this.preview_generation != generation {
                    return;
                }
                match result {
                    Ok(result) => {
                        if let Some(
                            PreviewRequest::Worktree { id, .. }
                            | PreviewRequest::Revision { id, .. },
                        ) = &mut this.preview_request
                        {
                            *id = None;
                        } else {
                            this.preview_request = None;
                        }
                        if close_form && form_id.is_some()
                            && this.form.as_ref().and_then(form::Form::first_field_id) == form_id
                        {
                            this.form = None;
                            cx.notify();
                        }
                        this.finish(output, root, result, window, cx);
                    }
                    Err(error) if error.kind != "cancelled" => {
                        this.preview_request = None;
                        this.set_error(error, certificate_target, cx);
                        cx.notify();
                    }
                    Err(_) => this.preview_request = None,
                }
            });
        })
        .detach();
    }

    fn repo(&self) -> Option<&Repo> {
        self.repos
            .iter()
            .find(|repo| Some(&repo.copy.root) == self.selected_root.as_ref())
    }

    fn focused_path(&self) -> String {
        self.active_path
            .clone()
            .or_else(|| self.selected_paths.first().cloned())
            .unwrap_or_else(|| ".".into())
    }

    fn show_status(&self, text: impl Into<SharedString>, error: bool, cx: &mut App) {
        self.status_toast(text, error, |toast| toast, cx);
    }

    fn status_toast(
        &self,
        text: impl Into<SharedString>,
        error: bool,
        configure: impl FnOnce(StatusToast) -> StatusToast + 'static,
        cx: &mut App,
    ) {
        let text = text.into();
        let text = util::truncate_and_trailoff(text.lines().next().unwrap_or_default(), 110);
        let workspace = self.workspace.clone();
        cx.defer(move |cx| {
            let _ = workspace.update(cx, |workspace, cx| {
                let toast = StatusToast::new(text, cx, |toast, _| {
                    configure(
                        toast
                            .icon(
                                Icon::new(if error {
                                    IconName::Warning
                                } else {
                                    IconName::Subversion
                                })
                                .size(IconSize::Small)
                                .color(if error {
                                    Color::Error
                                } else {
                                    Color::Muted
                                }),
                            )
                            .dismiss_button(true),
                    )
                });
                workspace.toggle_status_toast(toast, cx);
            });
        });
    }

    fn show_output_status(
        &self,
        text: impl Into<SharedString>,
        output: String,
        error: bool,
        cx: &mut App,
    ) {
        let workspace = self.workspace.clone();
        self.status_toast(
            text,
            error,
            move |toast| {
                toast.action(t!("SVN Output"), move |window, cx| {
                    preview::text(
                        workspace.clone(),
                        t!("SVN Output"),
                        output.clone(),
                        window,
                        cx,
                    );
                })
            },
            cx,
        );
    }

    fn set_error(
        &mut self,
        error: rpc::Error,
        certificate_target: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.last_output = Some(error.message.clone());
        self.error_kind = Some(error.kind);
        self.certificate_target = certificate_target;
        let panel = cx.weak_entity();
        if let Some(target) = self.certificate_recovery().map(str::to_owned) {
            self.status_toast(
                error.message,
                true,
                move |toast| {
                    toast.action(t!("Verify SVN Server Certificate"), move |window, cx| {
                        let _ = panel.update(cx, |panel, cx| {
                            if panel.certificate_recovery() == Some(target.as_str()) {
                                panel.certificate(window, cx);
                            }
                        });
                    })
                },
                cx,
            );
        } else if self.needs_credentials() {
            let root = self.selected_root.clone();
            self.status_toast(
                error.message,
                true,
                move |toast| {
                    toast.action(t!("SVN Credentials"), move |window, cx| {
                        let _ = panel.update(cx, |panel, cx| {
                            if panel.selected_root == root && panel.needs_credentials() {
                                panel.start_form(form::Kind::Credentials, window, cx);
                            }
                        });
                    })
                },
                cx,
            );
        } else {
            self.show_output_status(error.message.clone(), error.message, true, cx);
        }
    }

    fn certificate_recovery(&self) -> Option<&str> {
        self.certificate_target.as_deref().filter(|target| {
            self.error_kind.as_deref() == Some("certificate") && target.starts_with("https://")
        })
    }

    fn needs_credentials(&self) -> bool {
        self.error_kind.as_deref() == Some("authentication")
            && self.repo().is_some()
            && self
                .form
                .as_ref()
                .is_none_or(|form| form.kind != form::Kind::Checkout)
    }

    fn open_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.selected_root.clone() else {
            return;
        };
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        self.cancel_preview_request();
        let generation = self.preview_generation;
        let absolute_path = root.join(&path);
        let task = workspace.update(cx, |workspace, cx| {
            workspace.open_abs_path(
                absolute_path,
                OpenOptions {
                    visible: Some(OpenVisible::None),
                    focus: Some(true),
                    ..Default::default()
                },
                window,
                cx,
            )
        });
        self.preview_request = Some(PreviewRequest::File(cx.spawn_in(
            window,
            async move |this, cx| {
                if let Err(error) = task.await {
                    let _ = this.update(cx, |this, cx| {
                        if this.preview_generation == generation {
                            this.show_status(error.to_string(), true, cx);
                            cx.notify();
                        }
                    });
                }
            },
        )));
    }

    fn repository_params(&self, root: &std::path::Path, mut params: Value) -> Value {
        params["root"] = json!(root);
        if let Some(credentials) = self.credentials.get(root) {
            params["auth"] = json!(credentials);
        }
        params
    }

    fn run(
        &mut self,
        method: &'static str,
        mut params: Value,
        output: Output,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let checkout = matches!(output, Output::Checkout);
        let history_changed = method == "write"
            && matches!(
                params["operation"].as_str(),
                Some("commit" | "copy_branch" | "switch" | "update")
            );
        let discover = params["with_externals"].as_bool().unwrap_or(false);
        let root = self.selected_root.clone();
        if !checkout && root.is_none() {
            return;
        }
        if !checkout && let Some(root) = &root {
            params["root"] = json!(root);
        }
        if method == "write"
            && !checkout
            && params["dry_run"].as_bool() != Some(true)
            && !matches!(
                params["operation"].as_str(),
                Some("cleanup" | "lock" | "unlock" | "changelist")
            )
            && self.dirty(cx)
        {
            self.show_status(
                t!("Save open SVN files before running this operation."),
                true,
                cx,
            );
            cx.notify();
            return;
        }
        let credentials = if checkout {
            None
        } else {
            root.as_ref().and_then(|root| self.credentials.get(root))
        };
        if let Some(credentials) = credentials {
            params["auth"] = json!(credentials);
        }
        let certificate_target = params
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                params
                    .get("source")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .or_else(|| self.repo().map(|repo| repo.copy.url.clone()));
        if matches!(
            output,
            Output::Diff | Output::WorktreeDiff | Output::Conflict
        ) {
            self.request_preview(client, root, params, output, certificate_target, window, cx);
            return;
        }
        if method == "write" {
            self.cancel_commit_generation(cx);
        }
        self.busy = true;
        self.cancellation_requested = false;
        self.error_kind = None;
        self.certificate_target = None;
        self.revision += 1;
        if self.scanning {
            self.pending_refresh = Some(true);
        }
        self.scanning = false;
        self.refresh = None;
        self.deferred_refresh = None;
        let revision = self.revision;
        let close_form = params
            .as_object_mut()
            .and_then(|params| params.remove("close_form"))
            .and_then(|value| value.as_bool())
            == Some(true);
        let submitted_message = if matches!(output, Output::Commit) {
            params["message"].as_str().map(str::to_owned)
        } else {
            None
        };
        let checkout_credentials = if checkout {
            params["auth"]["username"]
                .as_str()
                .filter(|name| !name.is_empty())
                .map(|username| Credentials {
                    username: username.to_owned(),
                    password: params["auth"]["password"].as_str().map(str::to_owned),
                    remember: params["auth"]["remember"].as_bool().unwrap_or(false),
                })
        } else {
            None
        };
        let id = client.next_id();
        self.operation_id = Some(id);
        let task = cx.background_spawn(async move { client.request(id, method, params).await });
        self.operation = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if revision != this.revision {
                    return;
                }
                this.busy = false;
                this.operation_id = None;
                match result {
                    Ok(result) => {
                        if let Some(credentials) = checkout_credentials
                            && let Some(destination) = result["destination"].as_str()
                        {
                            this.history_requests
                                .invalidate(std::path::Path::new(destination));
                            this.credentials
                                .insert(PathBuf::from(destination), credentials);
                        }
                        if close_form {
                            this.form = None;
                        }
                        if let Some(message) = submitted_message
                            && this.commit_editor.read(cx).text(cx) == message
                        {
                            this.commit_editor.update(cx, |editor, cx| {
                                editor.set_text(String::new(), window, cx)
                            });
                        }
                        if history_changed && let Some(root) = &root {
                            this.reload_history_for(root, window, cx);
                        }
                        if history_changed {
                            this.refresh_editor_baselines(root.as_deref(), window, cx);
                        }
                        this.finish(output, root, result, window, cx);
                    }
                    Err(error) => {
                        if error.kind == "transport" {
                            this.client = None;
                            this.history_requests.clear();
                        }
                        this.set_error(error, certificate_target, cx);
                    }
                }
                if method == "write" && !matches!(output, Output::Checkout) {
                    this.refresh(discover, window, cx);
                } else if let Some(discover) = this.pending_refresh {
                    this.schedule_refresh(discover, window, cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn finish(
        &mut self,
        output: Output,
        root: Option<PathBuf>,
        result: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = result["text"].as_str().unwrap_or_default().to_owned();
        let warning = result["warning"].as_str().map(str::to_owned);
        match output {
            Output::Text => {
                preview::text(self.workspace.clone(), t!("SVN Output"), value, window, cx)
            }
            Output::Diff => {
                let parsed = serde_json::from_value::<preview::DiffResult>(result);
                let scope = preview::Scope::new(self.preview_generation, cx);
                match parsed {
                    Ok(diff) => {
                        preview::show_diff(self.workspace.clone(), scope, root, diff, window, cx)
                    }
                    Err(error) => self.set_error(decode_error(error), None, cx),
                }
            }
            Output::RevisionReview { revision } => match serde_json::from_value(result) {
                Ok(review) => {
                    if let Some(root) = root {
                        preview::revision_diff(
                            self.workspace.clone(),
                            preview::Scope::new(self.preview_generation, cx),
                            root,
                            revision,
                            review,
                            window,
                            cx,
                        );
                    }
                }
                Err(error) => {
                    self.preview_request = None;
                    self.set_error(decode_error(error), None, cx);
                    cx.notify();
                }
            },
            Output::WorktreeDiff => match serde_json::from_value(result) {
                Ok(diff) => {
                    if let Some(root) = root {
                        let scope = preview::Scope::new(self.preview_generation, cx);
                        preview::worktree_diff(
                            self.workspace.clone(),
                            scope,
                            root,
                            diff,
                            window,
                            cx,
                        );
                    }
                }
                Err(error) => {
                    self.preview_request = None;
                    self.set_error(decode_error(error), None, cx);
                    cx.notify();
                }
            },
            Output::Blame => match serde_json::from_value(result["blame"].clone()) {
                Ok(Some(blame)) => preview::blame(self.workspace.clone(), blame, window, cx),
                Ok(None) => {
                    preview::text(self.workspace.clone(), t!("SVN Blame"), value, window, cx)
                }
                Err(error) => self.set_error(decode_error(error), None, cx),
            },
            Output::Conflict => {
                if let Some(root) = root {
                    if let (Some(kind), Some(message)) = (
                        result["tree_preview_error"]["kind"].as_str(),
                        result["tree_preview_error"]["message"].as_str(),
                    ) {
                        let url = self.repo().map(|repo| repo.copy.url.clone());
                        self.set_error(
                            rpc::Error {
                                kind: kind.into(),
                                message: message.into(),
                            },
                            url,
                            cx,
                        );
                    }
                    let path = result["path"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| self.focused_path());
                    conflicts::open(
                        self.workspace.clone(),
                        cx.weak_entity(),
                        preview::Scope::new(self.preview_generation, cx),
                        root,
                        path,
                        result,
                        window,
                        cx,
                    );
                }
            }
            Output::Change | Output::Commit => {
                let status = value
                    .lines()
                    .last()
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| t!("SVN operation completed.").to_string());
                self.last_output = (!value.is_empty()).then(|| value.clone());
                if value.is_empty() {
                    self.show_status(status, false, cx);
                } else {
                    self.show_output_status(status, value, false, cx);
                }
            }
            Output::Checkout => {
                if let Some(path) = result["destination"].as_str() {
                    self.checkout_root = Some(PathBuf::from(path));
                    self.show_status(t!("Checkout complete"), false, cx);
                    if let Some(workspace) = self.workspace.upgrade() {
                        workspace.update(cx, |workspace, cx| {
                            workspace
                                .open_paths(
                                    vec![PathBuf::from(path)],
                                    OpenOptions {
                                        visible: Some(OpenVisible::All),
                                        ..Default::default()
                                    },
                                    None,
                                    window,
                                    cx,
                                )
                                .detach();
                        });
                    }
                    self.refresh(true, window, cx);
                }
            }
            Output::RemoteStatus => match serde_json::from_value::<Status>(result) {
                Ok(Status { copy, entries }) => {
                    let changed = entries
                        .iter()
                        .filter(|entry| entry.remote_item.is_some() || entry.remote_props.is_some())
                        .count();
                    if let Some(repo) = self
                        .repos
                        .iter_mut()
                        .find(|repo| Some(&repo.copy.root) == root.as_ref())
                    {
                        *repo = Repo {
                            copy,
                            entries,
                            error: None,
                        };
                    }
                    self.prune_selection();
                    self.show_status(format!("{} ({changed})", t!("Remote Change")), false, cx);
                }
                Err(error) => self.set_error(decode_error(error), None, cx),
            },
        }
        if let Some(warning) = warning {
            self.show_status(warning, true, cx);
            cx.notify();
        }
    }

    fn update_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.repos.is_empty() {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        if self
            .repos
            .iter()
            .any(|repo| self.dirty_in(&repo.copy.root, cx))
        {
            self.show_status(
                t!("Save open SVN files before running this operation."),
                true,
                cx,
            );
            cx.notify();
            return;
        }
        let repos = self
            .repos
            .iter()
            .map(|repo| repo.copy.root.clone())
            .collect::<Vec<_>>();
        // The window task exposes each current request ID to the cancel button.
        let credentials = self.credentials.clone();
        self.cancel_commit_generation(cx);
        self.busy = true;
        self.cancellation_requested = false;
        self.scanning = false;
        self.error_kind = None;
        self.certificate_target = None;
        self.revision += 1;
        self.refresh = None;
        self.deferred_refresh = None;
        let revision = self.revision;
        self.operation = Some(cx.spawn_in(window, async move |this, cx| {
            let mut report = Vec::new();
            let mut failure = None;
            for root in repos {
                let id = client.next_id();
                let continue_batch = this.update(cx, |this, cx| {
                    if this.revision != revision || this.cancellation_requested { return false; }
                    this.operation_id = Some(id);
                    cx.notify();
                    true
                }).unwrap_or(false);
                if !continue_batch {
                    break;
                }
                let mut params = json!({"root":root,"operation":"update"});
                if let Some(auth) = credentials.get(&root) {
                    params["auth"] = json!(auth);
                }
                match client.request(id, "write", params).await {
                    Ok(result) => report.push(format!("{}\n{}", root.display(), result["text"].as_str().unwrap_or_default())),
                    Err(error) => {
                        failure.get_or_insert_with(|| error.message.clone());
                        report.push(format!("{}\n{}", root.display(), error.message));
                        if matches!(error.kind.as_str(), "uncertain" | "cancelled" | "transport") {
                            break;
                        }
                    }
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                if this.revision != revision { return; }
                this.busy = false;
                this.operation_id = None;
                let output = report.join("\n\n");
                this.last_output = Some(output.clone());
                let message = if this.cancellation_requested {
                    t!("Operation was canceled")
                } else if let Some(error) = &failure {
                    error.clone().into()
                } else {
                    t!("SVN operation completed.")
                };
                this.show_output_status(message, output, failure.is_some(), cx);
                this.refresh_editor_baselines(None, window, cx);
                this.refresh(false, window, cx);
            });
        }));
        cx.notify();
    }

    fn select_change(&mut self, path: String, checked: bool, cx: &mut Context<Self>) {
        let linked = self
            .repo()
            .and_then(|repo| repo.entries.iter().find(|entry| entry.path == path))
            .and_then(|entry| entry.moved_from.clone().or_else(|| entry.moved_to.clone()));
        let mut changed = false;
        for path in std::iter::once(&path).chain(linked.as_ref()) {
            let updated = if checked {
                self.selected_paths.insert(path.clone())
            } else {
                self.selected_paths.remove(path)
            };
            changed |= updated;
        }
        if changed {
            self.cancel_commit_generation(cx);
        }
        self.active_path = Some(path);
        self.active_directory = None;
        cx.notify();
    }
}

fn decode_error(error: serde_json::Error) -> rpc::Error {
    rpc::Error {
        kind: "protocol".into(),
        message: format!("Invalid SVN provider response: {error}"),
    }
}

impl Focusable for SvnPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl EventEmitter<PanelEvent> for SvnPanel {}

impl Panel for SvnPanel {
    fn persistent_name() -> &'static str {
        "SVN Panel"
    }
    fn panel_key() -> &'static str {
        "svn-panel"
    }
    fn position(&self, _: &Window, _: &App) -> DockPosition {
        self.position
    }
    fn position_is_valid(&self, pos: DockPosition) -> bool {
        matches!(pos, DockPosition::Left | DockPosition::Right)
    }
    fn set_position(&mut self, pos: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        self.position = pos;
        cx.notify();
    }
    fn default_size(&self, _: &Window, _: &App) -> Pixels {
        px(360.)
    }
    fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
        self.enabled.then_some(IconName::Subversion)
    }
    fn icon_tooltip(&self, _: &Window, _: &App) -> Option<&'static str> {
        Some(n!("SVN Panel"))
    }
    fn icon_label(&self, _: &Window, _: &App) -> Option<String> {
        let count = self.repos.iter().map(Repo::change_count).sum::<usize>();
        (count > 0).then(|| count.to_string())
    }
    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(TogglePanel)
    }
    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if !active && self.repos.is_empty() && !self.busy {
            self.client = None;
        }
        if active && !self.scanning {
            if self.client.is_none() || self.repos.is_empty() {
                self.refresh(true, window, cx);
            } else if let Some(discover) = self.pending_refresh {
                self.schedule_refresh(discover, window, cx);
            }
        }
    }
    fn enabled(&self, _: &App) -> bool {
        self.enabled
    }
    fn activation_priority(&self) -> u32 {
        4
    }
}

impl SvnPanel {
    fn dirty(&self, cx: &App) -> bool {
        let Some(root) = self.selected_root.as_ref() else {
            return false;
        };
        self.dirty_in(root, cx)
    }

    fn dirty_in(&self, root: &std::path::Path, cx: &App) -> bool {
        let Some(workspace) = self.workspace.upgrade() else {
            return false;
        };
        let project = self.project.read(cx);
        workspace.read(cx).items(cx).any(|item| {
            item.is_dirty(cx)
                && item
                    .project_path(cx)
                    .and_then(|path| project.absolute_path(&path, cx))
                    .is_some_and(|path| path.starts_with(root))
        })
    }

    fn read_path(&self, cx: &App) -> String {
        if self.active_path.is_some() || !self.selected_paths.is_empty() {
            return self.focused_path();
        }
        self.selected_root
            .as_ref()
            .and_then(|root| {
                let workspace = self.workspace.upgrade()?;
                let item = workspace.read(cx).active_item(cx)?;
                let path = self
                    .project
                    .read(cx)
                    .absolute_path(&item.project_path(cx)?, cx)?;
                path.strip_prefix(root).ok()?.to_str().map(str::to_owned)
            })
            .unwrap_or_else(|| ".".into())
    }

    fn confirm(
        &mut self,
        title: SharedString,
        detail: String,
        method: &'static str,
        params: Value,
        output: Output,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.selected_root.clone();
        let answer = window.prompt(
            PromptLevel::Warning,
            title.as_ref(),
            Some(&detail),
            &[t!("Continue").as_ref(), t!("Cancel").as_ref()],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update_in(cx, |this, window, cx| {
                    if selected == this.selected_root {
                        this.run(method, params, output, window, cx);
                    }
                });
            }
        })
        .detach();
    }

    fn confirm_all_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let roots = self
            .repos
            .iter()
            .map(|repo| repo.copy.root.clone())
            .collect::<Vec<_>>();
        let detail = roots
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let answer = window.prompt(
            PromptLevel::Warning,
            &t!("Update All SVN Working Copies"),
            Some(&detail),
            &[t!("Continue").as_ref(), t!("Cancel").as_ref()],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update_in(cx, |this, window, cx| {
                    if this
                        .repos
                        .iter()
                        .map(|repo| &repo.copy.root)
                        .eq(roots.iter())
                    {
                        this.update_all(window, cx);
                    }
                });
            }
        })
        .detach();
    }

    fn simple_write(
        &mut self,
        operation: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(operation, "update" | "cleanup") {
            self.write_paths(operation, Vec::new(), window, cx);
            return;
        }
        self.write_paths(
            operation,
            self.path_operation_targets(operation, cx),
            window,
            cx,
        );
    }

    fn path_operation_targets(&self, operation: &str, cx: &App) -> Vec<String> {
        let path = self.read_path(cx);
        let mut paths = self.selected_paths.iter().cloned().collect::<Vec<_>>();
        if paths.is_empty() && path != "." {
            paths.push(path);
        }
        if let Some(repo) = self.repo() {
            paths.retain(|path| {
                let entry = repo.entries.iter().find(|entry| &entry.path == path);
                match operation {
                    "add" => entry.is_some_and(|entry| entry.item == "unversioned"),
                    "delete" => entry.is_none_or(|entry| {
                        !entry.file_external
                            && !entry.conflicted()
                            && !matches!(
                                entry.item.as_str(),
                                "unversioned" | "ignored" | "external" | "deleted"
                            )
                    }),
                    "revert" | "revert_tree" => entry.is_none_or(|entry| {
                        entry.committable() || entry.conflicted() || entry.item == "missing"
                    }),
                    "unlock" => entry.is_none_or(Change::can_unlock),
                    "changelist" => entry.is_none_or(|entry| entry.changelist.is_some()),
                    _ => true,
                }
            });
        }
        paths
    }

    fn write_paths(
        &mut self,
        operation: &'static str,
        paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo() else { return };
        if paths.is_empty() && !matches!(operation, "update" | "cleanup") {
            self.show_status(t!("Select at least one path."), true, cx);
            cx.notify();
            return;
        }
        let params =
            json!({"operation":operation,"paths":paths,"recursive":operation=="revert_tree"});
        let title = match operation {
            "update" => t!("SVN Update"),
            "delete" => t!("SVN Delete"),
            "revert" | "revert_tree" => t!("SVN Revert"),
            "cleanup" => t!("SVN Cleanup"),
            "unlock" => t!("SVN Unlock"),
            _ => {
                self.run("write", params, Output::Change, window, cx);
                return;
            }
        };
        let mut params = params;
        if operation == "revert_tree" {
            params["operation"] = json!("revert");
        }
        let detail = format!("{}\n{}", repo.copy.root.display(), paths.join("\n"));
        self.confirm(title, detail, "write", params, Output::Change, window, cx);
    }

    fn certificate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.certificate_recovery().map(str::to_owned) else {
            return;
        };
        self.interactive_task(
            "--verify-certificate",
            t!("Verify SVN Server Certificate"),
            vec![target],
            false,
            window,
            cx,
        );
    }

    fn interactive_resolve(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.selected_root.as_ref().and_then(|root| root.to_str()) else {
            return;
        };
        if self.dirty(cx) {
            self.show_status(
                t!("Save open SVN files before running this operation."),
                true,
                cx,
            );
            cx.notify();
            return;
        }
        self.interactive_task(
            "--resolve-interactively",
            t!("Resolve SVN Conflict"),
            vec![root.to_owned(), path],
            true,
            window,
            cx,
        );
    }

    fn interactive_task(
        &mut self,
        operation: &'static str,
        title: SharedString,
        args: Vec<String>,
        refresh: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use task::{RevealStrategy, Shell, SpawnInTerminal, TaskId};
        use terminal_view::terminal_panel::TerminalPanel;
        let (Some(provider), Some(workspace)) = (&self.provider, self.workspace.upgrade()) else {
            return;
        };
        let Some(terminal) = workspace.read(cx).panel::<TerminalPanel>(cx) else {
            return;
        };
        let binary = match provider.executable() {
            Ok(binary) => binary,
            Err(error) => {
                self.set_error(error, None, cx);
                return;
            }
        };
        fn quote(value: &str) -> String {
            format!("'{}'", value.replace('\'', "'\"'\"'"))
        }
        let terminal_task = SpawnInTerminal {
            id: TaskId(format!("wu-plus-subversion:{operation}")),
            full_label: title.to_string(),
            label: title.to_string(),
            command: Some(quote(&binary.to_string_lossy())),
            args: std::iter::once(operation.to_owned())
                .chain(args.iter().map(|arg| quote(arg)))
                .collect(),
            cwd: self.selected_root.clone(),
            reveal: RevealStrategy::Always,
            use_new_terminal: true,
            shell: Shell::Program("/bin/sh".into()),
            ..Default::default()
        };
        let task = terminal.update(cx, |panel, cx| panel.spawn_task(&terminal_task, window, cx));
        cx.spawn_in(window, async move |this, cx| {
            let terminal = task.await?;
            if refresh {
                terminal
                    .read_with(cx, |terminal, cx| terminal.wait_for_completed_task(cx))?
                    .await;
                this.update_in(cx, |this, window, cx| this.refresh(false, window, cx))?;
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }
}
