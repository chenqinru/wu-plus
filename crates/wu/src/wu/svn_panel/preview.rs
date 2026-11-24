use super::*;
use git_ui_core::file_diff_view::DiffHunkNavigation;
use gpui::SharedString;
use language::{Buffer, Capability};
use settings::Settings as _;
use std::{any::Any, path::Path};
use workspace::{Item, ItemNavHistory, Pane};

pub(super) fn view(
    title: &str,
    content: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Editor> {
    cx.new(|cx| {
        let buffer = cx.new(|cx| language::Buffer::local(content, cx));
        let mut editor = Editor::for_buffer(buffer, None, window, cx);
        editor
            .buffer()
            .update(cx, |buffer, cx| buffer.set_title(title.to_owned(), cx));
        editor.set_read_only(true);
        editor
    })
}

fn open_preview<T: Item>(
    workspace: &mut Workspace,
    item: Entity<T>,
    window: &mut Window,
    cx: &mut App,
) {
    workspace.active_pane().update(cx, |pane, cx| {
        pane.add_preview_item(Box::new(item), window, cx);
    });
}

#[derive(Clone)]
pub(super) struct Scope {
    panel: WeakEntity<SvnPanel>,
    generation: u64,
}

impl Scope {
    pub fn new(generation: u64, cx: &Context<SvnPanel>) -> Self {
        Self {
            panel: cx.weak_entity(),
            generation,
        }
    }

    pub(super) fn is_current(&self, cx: &App) -> bool {
        self.panel
            .read_with(cx, |panel, _| panel.preview_generation == self.generation)
            .unwrap_or(false)
    }

    fn remember(
        &self,
        pane: Entity<Pane>,
        item: gpui::EntityId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let scope = self.clone();
        let pane = pane.downgrade();
        window.defer(cx, move |_, cx| {
            let _ = scope.panel.update(cx, |panel, _| {
                if panel.preview_generation == scope.generation {
                    panel.preview_tab = Some((pane, item));
                }
            });
        });
    }

    pub(super) fn open<T: Item>(
        &self,
        workspace: &mut Workspace,
        item: Entity<T>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.remember(
            workspace.active_pane().clone(),
            item.entity_id(),
            window,
            cx,
        );
        open_preview(workspace, item, window, cx);
    }
}

pub(super) fn text(
    workspace: WeakEntity<Workspace>,
    title: SharedString,
    content: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(workspace) = workspace.upgrade() else {
        return;
    };
    let content = if content.trim().is_empty() {
        t!("No SVN output.").to_string()
    } else {
        content
    };
    let editor = view(title.as_ref(), &content, window, cx);
    workspace.update(cx, |workspace, cx| {
        workspace.add_item_to_active_pane(Box::new(editor), None, true, window, cx);
    });
}

#[derive(Deserialize)]
pub(super) struct Blame {
    path: String,
    content: String,
    lines: Vec<BlameLine>,
}

#[derive(Deserialize)]
struct BlameLine {
    line: u32,
    revision: String,
    author: String,
    date: String,
}

pub(super) fn blame(
    workspace: WeakEntity<Workspace>,
    blame: Blame,
    window: &mut Window,
    cx: &mut App,
) {
    use editor::Inlay;
    use language::Point;
    use project::{InlayHint, InlayHintLabel, InlayId, ResolveState};

    window
        .spawn(cx, async move |cx| {
            let languages = workspace.read_with(cx, |workspace, cx| {
                workspace.project().read(cx).languages().clone()
            })?;
            let language = languages
                .load_language_for_file_path(std::path::Path::new(&blame.path))
                .await
                .ok();
            workspace.update_in(cx, |workspace, window, cx| {
                let buffer = cx.new(|cx| {
                    let mut buffer = Buffer::local(blame.content, cx);
                    buffer.set_capability(Capability::ReadOnly, cx);
                    buffer.set_language_registry(languages.clone());
                    buffer.set_language(language, cx);
                    buffer
                });
                let item = cx.new(|cx| {
                    let mut editor = Editor::for_buffer(buffer.clone(), None, window, cx);
                    let title = format!("{} · {}", t!("SVN Blame"), blame.path);
                    editor
                        .buffer()
                        .update(cx, |buffer, cx| buffer.set_title(title, cx));
                    editor.set_read_only(true);
                    let single = buffer.read(cx).snapshot();
                    let multi = editor.buffer().read(cx).snapshot(cx);
                    let now = time::OffsetDateTime::now_utc();
                    let inlays = blame
                        .lines
                        .into_iter()
                        .enumerate()
                        .filter_map(|(index, line)| {
                            let row = line.line.checked_sub(1)?;
                            if row > single.max_point().row {
                                return None;
                            }
                            let point = Point::new(row, single.line_len(row));
                            let date = time::OffsetDateTime::parse(
                                &line.date,
                                &time::format_description::well_known::Rfc3339,
                            )
                            .map(|date| {
                                time_format::format_local_timestamp(
                                    date,
                                    now,
                                    time_format::TimestampFormat::Relative,
                                )
                            })
                            .unwrap_or_default();
                            let hint = InlayHint {
                                position: single.anchor_before(point),
                                label: InlayHintLabel::String(format!(
                                    "  r{} · {} · {date}",
                                    line.revision, line.author
                                )),
                                kind: None,
                                padding_left: false,
                                padding_right: false,
                                tooltip: None,
                                resolve_state: ResolveState::Resolved,
                            };
                            Some(Inlay::hint(
                                InlayId::Hint(index),
                                multi.anchor_before(point),
                                &hint,
                            ))
                        })
                        .collect();
                    editor.splice_inlays(&[], inlays, cx);
                    editor
                });
                open_preview(workspace, item, window, cx);
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DiffKind {
    #[default]
    Content,
    Properties,
    Path,
}

impl DiffKind {
    fn label(self) -> Option<SharedString> {
        match self {
            Self::Content => None,
            Self::Properties => Some(t!("SVN Property Changes")),
            Self::Path => Some(t!("SVN Path Changes")),
        }
    }
}

#[derive(Clone, Deserialize)]
struct DiffFile {
    path: String,
    base: String,
    head: String,
    from: String,
    to: String,
    #[serde(default)]
    kind: DiffKind,
    sort_order: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct DiffResult {
    #[serde(default)]
    path: String,
    file: Option<DiffFile>,
    details: Option<DiffFile>,
    reason: Option<String>,
}

pub(super) fn show_diff(
    workspace: WeakEntity<Workspace>,
    scope: Scope,
    root: Option<PathBuf>,
    comparison: DiffResult,
    window: &mut Window,
    cx: &mut App,
) {
    match (comparison.file, comparison.details) {
        (Some(file), supplement) => diff(workspace, scope, root, file, supplement, window, cx),
        (None, Some(details)) => diff(workspace, scope, root, details, None, window, cx),
        (None, None) => {
            let message = match comparison.reason.as_deref() {
                Some("directory") => t!("Open the changed files to review this directory."),
                Some("binary") => t!("No text diff is available for this file."),
                _ => t!("No text changes to compare."),
            };
            notice(
                workspace,
                scope,
                comparison.path.into(),
                message,
                window,
                cx,
            );
        }
    }
}

fn notice(
    workspace: WeakEntity<Workspace>,
    scope: Scope,
    title: SharedString,
    message: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(workspace) = workspace.upgrade() {
        let item = cx.new(|cx| DiffNotice {
            title,
            message,
            focus: cx.focus_handle(),
            nav_history: None,
        });
        workspace.update(cx, |workspace, cx| scope.open(workspace, item, window, cx));
    }
}

struct DiffNotice {
    title: SharedString,
    message: SharedString,
    focus: FocusHandle,
    nav_history: Option<ItemNavHistory>,
}

impl EventEmitter<()> for DiffNotice {}

impl Focusable for DiffNotice {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Item for DiffNotice {
    type Event = ();

    fn set_nav_history(&mut self, history: ItemNavHistory, _: &mut Window, _: &mut Context<Self>) {
        self.nav_history = Some(history);
    }

    fn deactivated(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(history) = &mut self.nav_history {
            history.push::<()>(None, None, cx);
        }
    }

    fn tab_content_text(&self, _: usize, _: &App) -> SharedString {
        if self.title.is_empty() {
            t!("View Diff")
        } else {
            self.title.clone()
        }
    }
}

impl Render for DiffNotice {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .items_center()
            .justify_center()
            .gap_2()
            .child(Label::new(self.title.clone()))
            .child(Label::new(self.message.clone()).color(Color::Muted))
    }
}

#[derive(Deserialize)]
pub(super) struct WorktreeDiff {
    files: Vec<DiffFile>,
    other: Vec<String>,
    limited: bool,
    stats: Option<DiffStat>,
    #[serde(flatten)]
    metadata: RevisionMetadata,
}

#[derive(Default, Deserialize)]
struct RevisionMetadata {
    #[serde(default)]
    author: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    message: String,
}

pub(super) fn worktree_is_active(workspace: &WeakEntity<Workspace>, cx: &App) -> bool {
    workspace
        .read_with(cx, |workspace, cx| {
            workspace
                .active_item_as::<WorkingCopyDiff>(cx)
                .is_some_and(|item| item.read(cx).historical_revision.is_none())
        })
        .unwrap_or(false)
}

pub(super) fn close_worktree_diff(
    workspace: &WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(workspace) = workspace.upgrade() else {
        return;
    };
    for pane in workspace.read(cx).panes().to_vec() {
        pane.update(cx, |pane, cx| {
            let item = pane
                .items_of_type::<WorkingCopyDiff>()
                .map(|item| item.entity_id())
                .collect::<Vec<_>>();
            for item in item {
                pane.close_item_by_id(item, workspace::SaveIntent::Close, window, cx)
                    .detach_and_log_err(cx);
            }
        });
    }
}

pub(super) fn reveal_worktree(
    workspace: &WeakEntity<Workspace>,
    root: &Path,
    revision: u64,
    target: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) -> Option<(bool, bool)> {
    let workspace = workspace.upgrade()?;
    workspace.update(cx, |workspace, cx| {
        let item = workspace
            .items_of_type::<WorkingCopyDiff>(cx)
            .find(|item| item.read(cx).historical_revision.is_none())?;
        let diff = item.read(cx);
        if diff.root != root || diff.revision != revision {
            return None;
        }
        if !diff.complete && target.is_none_or(|path| !diff.paths.contains_key(path)) {
            return None;
        }
        let complete = diff.complete;
        let revealed = reveal_worktree_item(workspace, &item, target, true, window, cx);
        Some((revealed, complete))
    })
}

fn reveal_worktree_item(
    workspace: &mut Workspace,
    item: &Entity<WorkingCopyDiff>,
    target: Option<&str>,
    record_jump: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    use editor::{SelectionEffects, scroll::Autoscroll};

    let diff = item.read(cx);
    let position = if let Some(target) = target {
        let position = diff
            .paths
            .get(target)
            .and_then(|key| diff.multibuffer.read(cx).location_for_path(key, cx))
            .or_else(|| diff.report_position(target, cx));
        let Some(position) = position else {
            return false;
        };
        position
    } else {
        editor::Anchor::Min
    };
    let editor = diff.editor.read(cx).rhs_editor().clone();
    let multibuffer = diff.multibuffer.clone();
    let historical = diff.historical_revision.is_some();
    let was_active = workspace
        .active_item(cx)
        .is_some_and(|active| active.item_id() == item.entity_id());
    if !was_active {
        workspace.activate_item(item, true, historical, window, cx);
    }
    editor.update(cx, |editor, cx| {
        if target.is_some()
            && let Some((anchor, _)) = multibuffer
                .read(cx)
                .snapshot(cx)
                .anchor_to_buffer_anchor(position)
        {
            editor.unfold_buffer(anchor.buffer_id, cx);
        }
        editor.change_selections(
            SelectionEffects::scroll(Autoscroll::focused()).nav_history(record_jump && was_active),
            window,
            cx,
            |selections| {
                selections.select_ranges([position..position]);
            },
        );
    });
    true
}

pub(super) fn worktree_diff(
    workspace: WeakEntity<Workspace>,
    scope: Scope,
    root: PathBuf,
    comparison: WorktreeDiff,
    window: &mut Window,
    cx: &mut App,
) {
    comparison_diff(workspace, scope, root, None, comparison, window, cx);
}

pub(super) fn revision_diff(
    workspace: WeakEntity<Workspace>,
    scope: Scope,
    root: PathBuf,
    revision: u64,
    comparison: WorktreeDiff,
    window: &mut Window,
    cx: &mut App,
) {
    comparison_diff(
        workspace,
        scope,
        root,
        Some(revision),
        comparison,
        window,
        cx,
    );
}

pub(super) fn reveal_revision(
    workspace: &WeakEntity<Workspace>,
    root: &Path,
    revision: u64,
    target: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) -> Option<bool> {
    workspace.upgrade()?.update(cx, |workspace, cx| {
        let item = workspace
            .items_of_type::<WorkingCopyDiff>(cx)
            .find(|item| {
                let item = item.read(cx);
                item.historical_revision == Some(revision) && item.root == root
            })?;
        let diff = item.read(cx);
        let complete = diff.complete;
        if !complete && target.is_none_or(|path| !diff.paths.contains_key(path)) {
            return None;
        }
        if !reveal_worktree_item(workspace, &item, target, true, window, cx) {
            reveal_worktree_item(workspace, &item, None, true, window, cx);
        }
        Some(complete)
    })
}

fn comparison_diff(
    workspace: WeakEntity<Workspace>,
    scope: Scope,
    root: PathBuf,
    historical_revision: Option<u64>,
    comparison: WorktreeDiff,
    window: &mut Window,
    cx: &mut App,
) {
    use editor::PathKey;
    use git_ui_core::file_diff_view::build_buffer_diff;
    use language::{OffsetRangeExt as _, Point};
    use util::rel_path::RelPath;

    let report_paths = if historical_revision.is_some() {
        comparison
            .other
            .iter()
            .enumerate()
            .map(|(index, path)| (path.trim_start_matches('/').to_owned(), index as u32 + 2))
            .collect()
    } else {
        BTreeMap::new()
    };
    window
        .spawn(cx, async move |cx| {
            let prepared = async {
                let (revision, order) = scope.panel.read_with(cx, |panel, _| {
                    let revision = match &panel.preview_request {
                        Some(PreviewRequest::Worktree { revision, .. }) => *revision,
                        _ => 0,
                    };
                    let order = panel
                        .repos
                        .iter()
                        .find(|repo| repo.copy.root == root)
                        .into_iter()
                        .flat_map(|repo| &repo.entries)
                        .enumerate()
                        .map(|(index, entry)| (entry.path.clone(), index as u64))
                        .collect::<BTreeMap<_, _>>();
                    (revision, order)
                })?;
                let (project, languages, loaded) = workspace.read_with(cx, |workspace, cx| {
                    let project = workspace.project().clone();
                    let languages = project.read(cx).languages().clone();
                    let loaded = workspace
                        .items_of_type::<WorkingCopyDiff>(cx)
                        .find_map(|item| {
                            let item = item.read(cx);
                            if !item.is_partial_for(&root, revision, historical_revision) {
                                return None;
                            }
                            let snapshot = item.multibuffer.read(cx).snapshot(cx);
                            Some(
                                snapshot
                                    .excerpts()
                                    .filter_map(|excerpt| {
                                        snapshot
                                            .path_for_buffer(excerpt.context.start.buffer_id)
                                            .cloned()
                                    })
                                    .collect::<BTreeSet<_>>(),
                            )
                        })
                        .unwrap_or_default();
                    (project, languages, loaded)
                })?;
                let mut paths = BTreeMap::new();
                let mut excerpts = Vec::new();
                for (index, file) in comparison.files.into_iter().enumerate() {
                    if !cx.update(|_, cx| scope.is_current(cx))? {
                        return anyhow::Ok(None);
                    }
                    let sort_prefix = if historical_revision.is_some() {
                        file.sort_order.unwrap_or(index as u64)
                    } else {
                        let position = order.get(&file.path).copied().unwrap_or(order.len() as u64);
                        position * 2 + u64::from(file.kind != DiffKind::Content)
                    };
                    let path = PathKey::with_sort_prefix(
                        sort_prefix,
                        if file.path == "." {
                            RelPath::empty_arc()
                        } else {
                            RelPath::from_unix_str(&file.path)?.into_arc()
                        },
                    );
                    if loaded.contains(&path) {
                        paths.entry(file.path).or_insert(path);
                        continue;
                    }
                    let language = if file.kind == DiffKind::Content {
                        languages
                            .load_language_for_file_path(std::path::Path::new(&file.path))
                            .await
                            .ok()
                    } else {
                        None
                    };
                    let absolute_path = root.join(&file.path);
                    let file_handle: Option<Arc<dyn language::File>> =
                        project.read_with(cx, |project, cx| {
                            if historical_revision.is_some() {
                                let (worktree, _) = project.find_worktree(&root, cx)?;
                                return Some(Arc::new(RevisionFile {
                                    path: path.path.clone(),
                                    worktree_id: worktree.read(cx).id(),
                                })
                                    as Arc<dyn language::File>);
                            }
                            project
                                .find_worktree(&absolute_path, cx)
                                .map(|(worktree, path)| {
                                    Arc::new(project::File {
                                        worktree,
                                        path,
                                        disk_state: language::DiskState::Historic {
                                            was_deleted: false,
                                        },
                                        entry_id: None,
                                        is_local: false,
                                        is_private: false,
                                    })
                                        as Arc<dyn language::File>
                                })
                        });
                    let file_label = file
                        .kind
                        .label()
                        .map(|label| format!("{} · {label}", file.path));
                    let make_buffer = |content: String, cx: &mut Context<Buffer>| {
                        let content = if let Some(label) = &file_label {
                            format!("{label}\n{content}")
                        } else {
                            content
                        };
                        let mut buffer = Buffer::local(content, cx);
                        if let Some(file) = file_handle.clone() {
                            buffer.file_updated(file, cx);
                        }
                        buffer.set_capability(Capability::ReadOnly, cx);
                        buffer.set_language_registry(languages.clone());
                        buffer.set_language(language.clone(), cx);
                        buffer
                    };
                    let old = cx.new(|cx| make_buffer(file.base, cx));
                    let new = cx.new(|cx| make_buffer(file.head, cx));
                    let diff = build_buffer_diff(&old, &new, cx).await?;
                    let ranges = cx.update(|_, cx| {
                        let buffer = new.read(cx).snapshot();
                        diff.read(cx)
                            .snapshot(cx)
                            .hunks(&buffer)
                            .map(|hunk| hunk.buffer_range.to_point(&buffer))
                            .collect::<Vec<_>>()
                    })?;
                    if !ranges.is_empty() {
                        paths.entry(file.path).or_insert_with(|| path.clone());
                        excerpts.push((path, new, diff, ranges));
                    }
                }
                anyhow::Ok(Some((project, paths, excerpts)))
            }
            .await;

            scope.panel.update_in(cx, |panel, window, cx| {
                if panel.preview_generation != scope.generation {
                    return;
                }
                let (revision, target, stats, focused, background) =
                    if historical_revision.is_some() {
                        let Some(PreviewRequest::Revision {
                            target,
                            focused,
                            background,
                            ..
                        }) = panel.preview_request.take()
                        else {
                            return;
                        };
                        (0, target, comparison.stats, focused, background)
                    } else {
                        let Some(PreviewRequest::Worktree {
                            revision,
                            target,
                            focused,
                            background,
                            ..
                        }) = panel.preview_request.take()
                        else {
                            return;
                        };
                        (
                            revision,
                            target,
                            Some(panel.diff_totals()),
                            focused,
                            background,
                        )
                    };
                let (project, paths, excerpts) = match prepared {
                    Ok(Some(prepared)) => prepared,
                    Ok(None) => return,
                    Err(error) => {
                        panel.show_status(error.to_string(), true, cx);
                        cx.notify();
                        return;
                    }
                };
                if historical_revision.is_none() && panel.revision != revision {
                    if !background || worktree_is_active(&workspace, cx) {
                        let target = if background {
                            panel.repo().and_then(|repo| {
                                repo.entries.iter().find_map(|entry| {
                                    (panel.active_path.as_ref() == Some(&entry.path)
                                        && entry.can_view_diff()
                                        && !entry.conflicted())
                                    .then(|| entry.moved_to.as_ref().unwrap_or(&entry.path).clone())
                                })
                            })
                        } else {
                            target
                        };
                        panel.show_worktree_diff(target, window, cx);
                    }
                    return;
                }
                if historical_revision.is_none()
                    && let Some(path) = target.as_ref().filter(|path| !paths.contains_key(*path))
                {
                    panel.open_path_diff(path.clone(), window, cx);
                    return;
                }
                if let Some(historical_revision) = historical_revision
                    && focused
                    && target
                        .as_ref()
                        .is_some_and(|path| !paths.contains_key(path))
                {
                    // Keep the revision's report for paths without a text excerpt.
                    panel.request_revision_review(
                        root,
                        historical_revision,
                        target,
                        false,
                        false,
                        window,
                        cx,
                    );
                    return;
                }
                let Some(workspace) = workspace.upgrade() else {
                    return;
                };
                let revealed = workspace.update(cx, |workspace, cx| {
                    let item = workspace.items_of_type::<WorkingCopyDiff>(cx).find(|item| {
                        let item = item.read(cx);
                        item.historical_revision == historical_revision
                            && (historical_revision.is_none() || item.root == root)
                    });
                    let merge = item.as_ref().is_some_and(|item| {
                        item.read(cx)
                            .is_partial_for(&root, revision, historical_revision)
                    });
                    if background && !merge {
                        return false;
                    }
                    let new_item = item.is_none();
                    if !merge
                        && let Some(item) = &item
                        && let Some(pane) = workspace
                            .panes()
                            .iter()
                            .find(|pane| pane.read(cx).index_for_item(item).is_some())
                    {
                        // The replacement creates fresh snapshot buffers. Old anchors can
                        // otherwise resolve by row to a different file or working copy.
                        pane.update(cx, |pane, cx| {
                            pane.nav_history_mut().remove_item(item.entity_id());
                            cx.notify();
                        });
                    }
                    let handle = cx.entity();
                    let item = item.unwrap_or_else(|| {
                        cx.new(|cx| {
                            WorkingCopyDiff::new(
                                project,
                                handle,
                                scope.panel.clone(),
                                root.clone(),
                                historical_revision,
                                window,
                                cx,
                            )
                        })
                    });
                    let view = item.read(cx);
                    let multibuffer = view.multibuffer.clone();
                    let editor = view.editor.clone();
                    let scroll = background.then(|| {
                        use editor::{RowExt as _, display_map::ToDisplayPoint as _};

                        let rhs = editor.read(cx).rhs_editor().clone();
                        let (snapshot, mut scroll) = rhs.update(cx, |editor, cx| {
                            let snapshot = editor.snapshot(window, cx).display_snapshot;
                            let scroll = editor.scroll_manager.native_anchor(&snapshot, cx);
                            (snapshot, scroll)
                        });
                        if scroll.anchor == editor::Anchor::Min {
                            // Anchor::Min would stay at the top of the entire diff when earlier
                            // files are inserted. Pin it to the currently visible excerpt.
                            scroll.anchor = snapshot.buffer_snapshot().anchor_before(Point::zero());
                            scroll.offset.y -=
                                scroll.anchor.to_display_point(&snapshot).row().as_f64();
                        }
                        (rhs, scroll)
                    });
                    let old_paths = if merge {
                        BTreeSet::new()
                    } else {
                        let snapshot = multibuffer.read(cx).snapshot(cx);
                        snapshot
                            .excerpts()
                            .filter_map(|excerpt| {
                                snapshot
                                    .path_for_buffer(excerpt.context.start.buffer_id)
                                    .cloned()
                            })
                            .collect::<BTreeSet<_>>()
                    };
                    editor.update(cx, |editor, cx| {
                        for path in old_paths {
                            editor.remove_excerpts_for_path(path, cx);
                        }
                        for (path, buffer, diff, ranges) in excerpts {
                            editor.update_excerpts_for_path(path, buffer, ranges, 3, diff, cx);
                        }
                    });
                    let report = if !focused && !comparison.other.is_empty() {
                        let heading = if comparison.limited && historical_revision.is_none() {
                            t!("Open the remaining changes individually from the SVN panel.")
                        } else {
                            t!("Changes without a text diff")
                        };
                        Some(format!("{heading}\n\n{}", comparison.other.join("\n")))
                    } else if !focused && paths.is_empty() && !merge {
                        Some(t!("No SVN changes.").to_string())
                    } else {
                        None
                    };
                    let report_buffer = multibuffer.update(cx, |multibuffer, cx| {
                        if let Some(report) = report {
                            let details = cx.new(|cx| {
                                let mut buffer = Buffer::local(report, cx);
                                buffer.set_capability(Capability::ReadOnly, cx);
                                buffer
                            });
                            let end = details.read(cx).snapshot().max_point();
                            multibuffer.set_excerpts_for_path(
                                PathKey::sorted(u64::MAX),
                                details.clone(),
                                [Point::zero()..end],
                                0,
                                cx,
                            );
                            Some(details)
                        } else {
                            None
                        }
                    });
                    if let Some((editor, scroll)) = scroll {
                        editor.update(cx, |editor, cx| {
                            editor.set_scroll_anchor(scroll, window, cx)
                        });
                    }
                    item.update(cx, |item, cx| {
                        item.root = root.clone();
                        item.revision = revision;
                        item.complete = !focused;
                        if merge {
                            item.paths.extend(paths);
                        } else {
                            item.paths = paths;
                        }
                        item.stats = stats;
                        item.report = report_buffer.map(|buffer| (buffer, report_paths));
                        item.metadata = comparison.metadata;
                        cx.notify();
                    });
                    if new_item {
                        workspace.add_item_to_active_pane(
                            Box::new(item.clone()),
                            None,
                            historical_revision.is_some(),
                            window,
                            cx,
                        );
                    }
                    if background {
                        return true;
                    }
                    let revealed = reveal_worktree_item(
                        workspace,
                        &item,
                        target.as_deref(),
                        merge && focused,
                        window,
                        cx,
                    );
                    if !revealed && historical_revision.is_some() {
                        reveal_worktree_item(workspace, &item, None, false, window, cx);
                    }
                    revealed
                });
                if focused && revealed {
                    if let Some(revision) = historical_revision {
                        panel
                            .request_revision_review(root, revision, None, false, true, window, cx);
                    } else {
                        panel.load_worktree_overview(window, cx);
                    }
                }
                if historical_revision.is_none()
                    && !revealed
                    && let Some(path) = target
                {
                    panel.open_path_diff(path, window, cx);
                }
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
}

struct WorkingCopyDiff {
    title: SharedString,
    editor: Entity<editor::SplittableEditor>,
    multibuffer: Entity<editor::MultiBuffer>,
    root: PathBuf,
    revision: u64,
    complete: bool,
    paths: BTreeMap<String, editor::PathKey>,
    stats: Option<DiffStat>,
    historical_revision: Option<u64>,
    metadata: RevisionMetadata,
    message_expanded: bool,
    report: Option<(Entity<Buffer>, BTreeMap<String, u32>)>,
    panel: WeakEntity<SvnPanel>,
    workspace: WeakEntity<Workspace>,
    _open_excerpts: Subscription,
}

struct RevisionFile {
    path: Arc<util::rel_path::RelPath>,
    worktree_id: settings::WorktreeId,
}

impl language::File for RevisionFile {
    fn as_local(&self) -> Option<&dyn language::LocalFile> {
        None
    }

    fn disk_state(&self) -> language::DiskState {
        language::DiskState::Historic { was_deleted: false }
    }

    fn path(&self) -> &Arc<util::rel_path::RelPath> {
        &self.path
    }

    fn full_path(&self, _: &App) -> PathBuf {
        self.path.as_std_path().to_owned()
    }

    fn path_style(&self, _: &App) -> util::paths::PathStyle {
        util::paths::PathStyle::local()
    }

    fn file_name<'a>(&'a self, _: &'a App) -> &'a str {
        self.path.file_name().unwrap_or_default()
    }

    fn worktree_id(&self, _: &App) -> settings::WorktreeId {
        self.worktree_id
    }

    fn to_proto(&self, _: &App) -> language::proto::File {
        language::proto::File {
            path: self.path.as_unix_str().to_owned(),
            worktree_id: self.worktree_id.to_proto(),
            is_historic: true,
            ..Default::default()
        }
    }

    fn is_private(&self) -> bool {
        false
    }

    fn can_open(&self) -> bool {
        false
    }
}

impl WorkingCopyDiff {
    fn new(
        project: Entity<project::Project>,
        workspace: Entity<Workspace>,
        panel: WeakEntity<SvnPanel>,
        root: PathBuf,
        historical_revision: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title: SharedString = historical_revision.map_or_else(
            || t!("SVN Working Copy Diff"),
            |revision| {
                let name = root
                    .file_name()
                    .unwrap_or(root.as_os_str())
                    .to_string_lossy();
                format!("r{revision} · {name}").into()
            },
        );
        let multibuffer = cx.new(|cx| {
            let mut buffer = editor::MultiBuffer::new(Capability::ReadOnly);
            buffer.set_title(title.to_string(), cx);
            buffer.set_all_diff_hunks_expanded(cx);
            buffer
        });
        let weak_workspace = workspace.downgrade();
        let editor = cx.new(|cx| {
            let editor = editor::SplittableEditor::new(
                editor::EditorSettings::get_global(cx).diff_view_style,
                multibuffer.clone(),
                project,
                workspace,
                window,
                cx,
            );
            editor
                .set_diff_hunk_renderer(Some(Arc::new(editor::HiddenUnstagedDiffHunkRenderer)), cx);
            editor.rhs_editor().update(cx, |editor, cx| {
                editor.set_read_only(true);
                editor.set_delegate_open_excerpts(true);
                editor.disable_diagnostics(cx);
            });
            editor
        });
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |item: &mut WorkingCopyDiff, _, event: &editor::EditorEvent, window, cx| {
                if let editor::EditorEvent::OpenExcerptsRequested {
                    selections_by_buffer,
                    split,
                } = event
                {
                    let paths = {
                        let rhs = item.editor.read(cx).rhs_editor().read(cx);
                        let buffer = rhs.buffer().read(cx);
                        selections_by_buffer
                            .keys()
                            .filter_map(|id| {
                                let buffer = buffer.buffer(*id)?.read(cx);
                                let file = project::File::from_dyn(buffer.file())?;
                                Some(file.worktree.read(cx).absolutize(&file.path))
                            })
                            .collect::<BTreeSet<_>>()
                    };
                    item.open_working_files(paths, *split, window, cx);
                }
            },
        );
        Self {
            title,
            editor,
            multibuffer,
            root,
            revision: 0,
            complete: false,
            paths: BTreeMap::new(),
            stats: None,
            historical_revision,
            metadata: RevisionMetadata::default(),
            message_expanded: false,
            report: None,
            panel,
            workspace: weak_workspace,
            _open_excerpts: subscription,
        }
    }

    fn is_partial_for(&self, root: &Path, revision: u64, historical_revision: Option<u64>) -> bool {
        self.historical_revision == historical_revision
            && !self.complete
            && self.root == root
            && self.revision == revision
    }

    fn report_position(&self, target: &str, cx: &App) -> Option<editor::Anchor> {
        let (buffer, paths) = self.report.as_ref()?;
        let row = *paths.get(target)?;
        let anchor = buffer
            .read(cx)
            .snapshot()
            .anchor_before(language::Point::new(row, 0));
        self.multibuffer
            .read(cx)
            .snapshot(cx)
            .anchor_in_excerpt(anchor)
    }

    fn current_path(&self, cx: &App) -> Option<String> {
        let editor = self.editor.read(cx).rhs_editor().read(cx);
        let buffer = editor.active_buffer(cx)?;
        let file = project::File::from_dyn(buffer.read(cx).file())?;
        let absolute = file.worktree.read(cx).absolutize(&file.path);
        let path = absolute.strip_prefix(&self.root).ok()?;
        Some(if path.as_os_str().is_empty() {
            ".".into()
        } else {
            super::svn_path_string(path)?
        })
    }

    fn open_working_files(
        &self,
        paths: BTreeSet<PathBuf>,
        split: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.historical_revision.is_some() {
            return;
        }
        let workspace = self.workspace.clone();
        window
            .spawn(cx, async move |cx| {
                for path in paths {
                    if !smol::fs::symlink_metadata(&path)
                        .await
                        .is_ok_and(|entry| entry.is_file())
                    {
                        continue;
                    }
                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            if split {
                                workspace.split_abs_path(path, false, window, cx)
                            } else {
                                workspace.open_abs_path(
                                    path,
                                    OpenOptions {
                                        visible: Some(OpenVisible::None),
                                        ..Default::default()
                                    },
                                    window,
                                    cx,
                                )
                            }
                        })?
                        .await?;
                }
                anyhow::Ok(())
            })
            .detach_and_log_err(cx);
    }
}

impl EventEmitter<()> for WorkingCopyDiff {}

impl Focusable for WorkingCopyDiff {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl Item for WorkingCopyDiff {
    type Event = ();

    fn set_nav_history(&mut self, history: ItemNavHistory, _: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx).rhs_editor().clone();
        editor.update(cx, |editor, _| editor.set_nav_history(Some(history)));
    }

    fn deactivated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx).rhs_editor().clone();
        editor.update(cx, |editor, cx| editor.deactivated(window, cx));
    }

    fn navigate(
        &mut self,
        data: Arc<dyn Any + Send>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let editor = self.editor.read(cx).rhs_editor().clone();
        let navigated = editor.update(cx, |editor, cx| editor.navigate(data, window, cx));
        if self.historical_revision.is_none()
            && let Some(path) = self.current_path(cx)
        {
            let panel = self.panel.clone();
            let root = self.root.clone();
            cx.defer(move |cx| {
                let _ = panel.update(cx, |panel, cx| {
                    if panel.selected_root.as_ref() == Some(&root)
                        && !panel.history.visible
                        && panel
                            .repo()
                            .is_some_and(|repo| repo.entries.iter().any(|entry| entry.path == path))
                    {
                        panel.active_path = Some(path);
                        panel.active_directory = None;
                        cx.notify();
                    }
                });
            });
        }
        navigated
    }

    fn tab_content_text(&self, _: usize, _: &App) -> SharedString {
        if let Some(revision) = self.historical_revision
            && let Some(subject) = self.metadata.message.lines().next()
            && !subject.is_empty()
        {
            let subject = util::truncate_and_trailoff(subject, 20);
            return format!("r{revision} — {subject}").into();
        }
        self.title.clone()
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<Icon> {
        Some(
            Icon::new(if self.historical_revision.is_some() {
                IconName::GitCommit
            } else {
                IconName::Diff
            })
            .color(Color::Muted),
        )
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        self.historical_revision.map(|_| self.title.clone())
    }

    fn act_as_type<'a>(
        &'a self,
        type_id: std::any::TypeId,
        handle: &'a Entity<Self>,
        cx: &'a App,
    ) -> Option<gpui::AnyEntity> {
        if type_id == std::any::TypeId::of::<Self>() {
            Some(handle.clone().into())
        } else {
            self.editor.read(cx).act_as_type(type_id, &self.editor, cx)
        }
    }

    fn added_to_workspace(
        &mut self,
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.added_to_workspace(workspace, window, cx)
        });
    }

    fn as_searchable(
        &self,
        _: &Entity<Self>,
        cx: &App,
    ) -> Option<Box<dyn workspace::searchable::SearchableItemHandle>> {
        self.editor.read(cx).as_searchable(&self.editor, cx)
    }
}

impl Render for WorkingCopyDiff {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self.editor.clone();
        v_flex()
            .size_full()
            .when_some(self.historical_revision, |view, revision| {
                view.child(self.revision_header(revision, cx))
            })
            .child(div().flex_1().min_h_0().child(editor))
    }
}

impl WorkingCopyDiff {
    fn revision_header(&self, revision: u64, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().colors().border_variant;
        let avatar = cx.theme().colors().element_background;
        let date = time::OffsetDateTime::parse(
            &self.metadata.date,
            &time::format_description::well_known::Rfc3339,
        )
        .map(|date| {
            time_format::format_localized_timestamp(
                date,
                time::OffsetDateTime::now_utc(),
                time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC),
                time_format::TimestampFormat::MediumAbsolute,
            )
        })
        .unwrap_or_default();
        let message = self.metadata.message.trim();
        let more = message.contains('\n');
        let message = if self.message_expanded {
            message
        } else {
            message.lines().next().unwrap_or_default()
        };
        let number = format!("r{revision}");
        v_flex()
            .w_full()
            .flex_none()
            .py_2p5()
            .gap_2()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .pr_2p5()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .child(
                                h_flex().w(rems(4.)).flex_none().justify_center().child(
                                    h_flex()
                                        .size(rems(2.5))
                                        .rounded_full()
                                        .justify_center()
                                        .bg(avatar)
                                        .child(
                                            Icon::new(IconName::Person)
                                                .size(IconSize::Small)
                                                .color(Color::Muted),
                                        ),
                                ),
                            )
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .min_w_0()
                                            .gap_1()
                                            .child(
                                                Label::new(self.metadata.author.clone()).truncate(),
                                            )
                                            .when(more, |row| {
                                                row.child(
                                                    ui::Disclosure::new(
                                                        "svn-commit-message-disclosure",
                                                        self.message_expanded,
                                                    )
                                                    .closed_icon(IconName::ExpandVertical)
                                                    .opened_icon(IconName::FoldVertical)
                                                    .tooltip(Tooltip::text(
                                                        if self.message_expanded {
                                                            t!("Fold Commit Description")
                                                        } else {
                                                            t!("Expand Commit Description")
                                                        },
                                                    ))
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.message_expanded =
                                                            !this.message_expanded;
                                                        cx.notify();
                                                    })),
                                                )
                                            }),
                                    )
                                    .child(
                                        Label::new(date).color(Color::Muted).size(LabelSize::Small),
                                    ),
                            ),
                    )
                    .child(
                        Button::new("svn-copy-commit-revision", number.clone())
                            .start_icon(
                                Icon::new(IconName::Copy)
                                    .size(IconSize::Small)
                                    .color(Color::Muted),
                            )
                            .tooltip(Tooltip::text(t!("Copy")))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    number.clone(),
                                ));
                            }),
                    ),
            )
            .when(!message.is_empty(), |header| {
                header.child(
                    div()
                        .id("svn-commit-description")
                        .pl(rems(4.5))
                        .pr_2p5()
                        .max_h(rems(12.))
                        .overflow_y_scroll()
                        .child(Label::new(message.to_owned())),
                )
            })
    }
}

#[derive(Default)]
pub(in crate::wu) struct WorktreeDiffToolbar {
    item: Option<WeakEntity<WorkingCopyDiff>>,
    subscriptions: Vec<Subscription>,
}

impl WorktreeDiffToolbar {
    fn update_panel(
        &self,
        cx: &mut App,
        update: impl FnOnce(&mut SvnPanel, &mut Context<SvnPanel>),
    ) {
        let Some(item) = self.item.as_ref().and_then(WeakEntity::upgrade) else {
            return;
        };
        let diff = item.read(cx);
        let root = diff.root.clone();
        if let Some(panel) = diff.panel.upgrade() {
            panel.update(cx, |panel, cx| {
                if panel.selected_root.as_ref() == Some(&root)
                    && !panel.busy
                    && panel.client.is_some()
                {
                    update(panel, cx);
                }
            });
        }
    }
}

impl EventEmitter<workspace::ToolbarItemEvent> for WorktreeDiffToolbar {}

impl workspace::ToolbarItemView for WorktreeDiffToolbar {
    fn set_active_pane_item(
        &mut self,
        active_item: Option<&dyn workspace::ItemHandle>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> workspace::ToolbarItemLocation {
        let item = active_item.and_then(|item| item.act_as::<WorkingCopyDiff>(cx));
        self.item = item.as_ref().map(Entity::downgrade);
        self.subscriptions.clear();
        if let Some(item) = item {
            let editor = item.read(cx).editor.clone();
            self.subscriptions
                .push(cx.observe(&item, |_, _, cx| cx.notify()));
            self.subscriptions
                .push(cx.subscribe(&editor, |_, _, event, cx| {
                    if matches!(event, editor::EditorEvent::SelectionsChanged { .. }) {
                        cx.notify();
                    }
                }));
            if let Some(panel) = item.read(cx).panel.upgrade() {
                self.subscriptions
                    .push(cx.observe(&panel, |_, _, cx| cx.notify()));
            }
            workspace::ToolbarItemLocation::PrimaryRight
        } else {
            workspace::ToolbarItemLocation::Hidden
        }
    }
}

impl Render for WorktreeDiffToolbar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::Divider;

        let Some(item) = self.item.as_ref().and_then(WeakEntity::upgrade) else {
            return gpui::Empty.into_any_element();
        };
        let diff = item.read(cx);
        let stats = diff.stats;
        if diff.historical_revision.is_some() {
            let focus = item.focus_handle(cx);
            return h_flex()
                .gap_1()
                .when_some(
                    stats.filter(|stats| stats.added + stats.deleted > 0),
                    |toolbar, stats| {
                        toolbar
                            .child(ui::DiffStat::new(
                                "svn-commit-stat",
                                stats.added,
                                stats.deleted,
                            ))
                            .child(Divider::vertical())
                    },
                )
                .child(
                    IconButton::new("svn-commit-search", IconName::MagnifyingGlass)
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::for_action_title_in(
                            t!("Buffer Search"),
                            &wu_actions::buffer_search::Deploy::find(),
                            &focus,
                        ))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(
                                Box::new(wu_actions::buffer_search::Deploy::find()),
                                cx,
                            );
                        }),
                )
                .into_any_element();
        }
        let multibuffer = diff.multibuffer.read(cx);
        let can_navigate = multibuffer.snapshot(cx).diff_hunks().next().is_some();
        let show_stats = !multibuffer.is_empty();
        let panel = diff.panel.upgrade();
        let panel = panel
            .as_ref()
            .map(|panel| panel.read(cx))
            .filter(|panel| panel.selected_root.as_ref() == Some(&diff.root));
        let paths = panel.map_or_else(Vec::new, SvnPanel::committable_paths);
        let path = diff.current_path(cx).filter(|path| paths.contains(path));
        let ready = panel.is_some_and(|panel| !panel.busy && panel.client.is_some());
        let checked = path
            .as_ref()
            .is_some_and(|path| panel.is_some_and(|panel| panel.selected_paths.contains(path)));
        let all_checked = !paths.is_empty()
            && panel
                .is_some_and(|panel| paths.iter().all(|path| panel.selected_paths.contains(path)));
        let can_commit = ready && panel.is_some_and(|panel| !panel.commit_selection().is_empty());

        h_flex()
            .my_neg_1()
            .py_1()
            .gap_1p5()
            .flex_wrap()
            .justify_between()
            .when_some(stats.filter(|_| show_stats), |toolbar, stats| {
                toolbar
                    .child(ui::DiffStat::new(
                        "svn-worktree-stat",
                        stats.added,
                        stats.deleted,
                    ))
                    .child(Divider::vertical().ml_1())
            })
            .child(DiffHunkNavigation::new(
                diff.editor.read(cx).rhs_editor().focus_handle(cx),
                can_navigate,
            ))
            .child(Divider::vertical())
            .child(
                Button::new("svn-worktree-select", t!("Include in SVN Commit"))
                    .toggle_state(checked)
                    .disabled(!ready || path.is_none())
                    .tooltip(Tooltip::text(t!("Include in SVN Commit")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(path) = &path {
                            this.update_panel(cx, |panel, cx| {
                                if panel.repo().is_some_and(|repo| {
                                    repo.entries
                                        .iter()
                                        .any(|entry| entry.path == *path && entry.committable())
                                }) {
                                    panel.select_change(
                                        path.clone(),
                                        !panel.selected_paths.contains(path),
                                        cx,
                                    );
                                }
                            });
                        }
                    })),
            )
            .child(Divider::vertical())
            .child(
                Button::new(
                    "svn-worktree-select-all",
                    if all_checked {
                        t!("Clear Selection")
                    } else {
                        t!("Select All")
                    },
                )
                .width(rems(5.))
                .disabled(!ready || paths.is_empty())
                .tooltip(Tooltip::text(t!("Select changes for the next SVN commit")))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.update_panel(cx, |panel, cx| {
                        let paths = panel.committable_paths();
                        let all_checked =
                            paths.iter().all(|path| panel.selected_paths.contains(path));
                        panel.select_paths(paths, !all_checked, cx);
                    });
                })),
            )
            .child(Divider::vertical())
            .child(
                Button::new("svn-worktree-commit", t!("Commit"))
                    .disabled(!can_commit)
                    .tooltip(Tooltip::text(t!("Commit Selected Changes")))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.update_panel(cx, |panel, cx| {
                            if !panel.commit_selection().is_empty() {
                                panel.open_commit_editor(&git::ExpandCommitEditor, window, cx);
                            }
                        });
                    })),
            )
            .into_any_element()
    }
}

fn diff(
    workspace: WeakEntity<Workspace>,
    scope: Scope,
    root: Option<PathBuf>,
    file: DiffFile,
    supplement: Option<DiffFile>,
    window: &mut Window,
    cx: &mut App,
) {
    use git_ui_core::file_diff_view::{FileDiffView, build_buffer_diff};
    window
        .spawn(cx, async move |cx| {
            let (project, languages) = workspace.read_with(cx, |workspace, cx| {
                let project = workspace.project().clone();
                let languages = project.read(cx).languages().clone();
                (project, languages)
            })?;
            let language = if file.kind == DiffKind::Content {
                languages
                    .load_language_for_file_path(std::path::Path::new(&file.path))
                    .await
                    .ok()
            } else {
                None
            };
            let snapshot = |content: String, cx: &mut gpui::Context<Buffer>| {
                let mut buffer = Buffer::local(content, cx);
                buffer.set_capability(Capability::ReadOnly, cx);
                buffer.set_language_registry(languages.clone());
                buffer.set_language(language.clone(), cx);
                buffer
            };
            let counterpart = supplement.as_ref().map(|_| file.clone());
            let working = match root
                .clone()
                .filter(|_| file.to == "WORKING" && file.kind == DiffKind::Content)
            {
                Some(root) => {
                    let path = root.join(&file.path);
                    let open = match smol::fs::symlink_metadata(&path).await {
                        Ok(metadata) => metadata.is_file(),
                        Err(_) => false,
                    };
                    if open {
                        project
                            .update(cx, |project, cx| project.open_local_buffer(&path, cx))
                            .await
                            .ok()
                    } else {
                        None
                    }
                }
                None => None,
            };
            let old_buffer = cx.new(|cx| snapshot(file.base, cx));
            let new_buffer = working.unwrap_or_else(|| cx.new(|cx| snapshot(file.head, cx)));
            let diff = build_buffer_diff(&old_buffer, &new_buffer, cx).await?;

            if !cx.update(|_, cx| scope.is_current(cx))? {
                return Ok(());
            }
            workspace.update_in(cx, |workbench, window, cx| {
                let entity = cx.entity();
                let title = file.kind.label().map_or(file.path.clone(), |label| {
                    format!("{} · {label}", file.path)
                });
                let item = cx.new(|cx| {
                    let view = FileDiffView::new(
                        old_buffer, new_buffer, diff, project, entity, window, cx,
                    )
                    .with_snapshot_labels(title, file.from, file.to, cx);
                    if let Some((details, counterpart)) = supplement.zip(counterpart) {
                        let panel = scope.panel.clone();
                        let workspace = workspace.clone();
                        let label = details.kind.label().unwrap_or_else(|| t!("View Diff"));
                        view.with_snapshot_action(label, move |window, cx| {
                            let Ok(Some(scope)) = panel.update(cx, |this, cx| {
                                if !this.enabled {
                                    return None;
                                }
                                this.cancel_preview_request();
                                Some(Scope::new(this.preview_generation, cx))
                            }) else {
                                return;
                            };
                            self::diff(
                                workspace.clone(),
                                scope,
                                root.clone(),
                                details.clone(),
                                Some(counterpart.clone()),
                                window,
                                cx,
                            );
                        })
                    } else {
                        view
                    }
                });
                scope.open(workbench, item, window, cx);
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
}
