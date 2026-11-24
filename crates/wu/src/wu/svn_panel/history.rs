use super::*;
use futures::future::WeakShared;
use gpui::{DefiniteLength, MouseButton, ScrollStrategy, SharedString};
use std::path::Path;
use ui::{
    ColumnWidthConfig, CommonAnimationExt as _, ContextMenu, HighlightedLabel,
    RedistributableColumnsState, Table, TableInteractionState, TableResizeBehavior,
    WithScrollbar as _, right_click_menu,
};
use workspace::{Item, ItemNavHistory};

#[derive(Default)]
pub(super) struct PanelHistory {
    pub visible: bool,
    repositories: BTreeMap<PathBuf, History>,
    filtered: Option<(String, History)>,
}

impl PanelHistory {
    pub(super) fn clear(&mut self) {
        self.repositories.clear();
        self.filtered = None;
    }

    pub(super) fn reset_view(&mut self, root: Option<&Path>) {
        self.filtered = None;
        if let Some(history) = root.and_then(|root| self.repositories.get_mut(root)) {
            history.selected = None;
        }
    }

    fn active(&self, root: &Path) -> Option<&History> {
        self.filtered
            .as_ref()
            .map(|(_, history)| history)
            .or_else(|| self.repositories.get(root))
    }

    fn state_mut(&mut self, root: &Path, filtered: bool) -> Option<&mut History> {
        if filtered {
            self.filtered.as_mut().map(|(_, history)| history)
        } else {
            self.repositories.get_mut(root)
        }
    }

}

#[derive(Default)]
struct History {
    entries: Vec<Revision>,
    selected: Option<u64>,
    next_before: Option<u64>,
    loaded: bool,
    failed: bool,
    loading: Option<Task<()>>,
    interactive: bool,
}

#[derive(Clone, Deserialize)]
pub(super) struct Page {
    revisions: Vec<Revision>,
    next_before: Option<u64>,
}

#[derive(Clone, Deserialize)]
struct Revision {
    revision: u64,
    author: String,
    date: String,
    message: String,
    paths: Vec<ChangedPath>,
}

#[derive(Clone, Deserialize)]
struct ChangedPath {
    path: String,
    kind: String,
    action: String,
}

type HistoryTask = Task<Result<Page, rpc::Error>>;

struct HistoryFetch {
    client: Arc<rpc::Client>,
    params: Value,
    task: Shared<HistoryTask>,
}

impl HistoryFetch {
    async fn load(
        self,
        panel: &WeakEntity<SvnPanel>,
        interactive: impl Fn(&SvnPanel) -> bool,
        cx: &mut gpui::AsyncWindowContext,
    ) -> Result<Page, rpc::Error> {
        match self.task.await {
            Err(error) if error.kind == "authentication"
                && panel.read_with(cx, |panel, _| interactive(panel)).unwrap_or(false) =>
            {
                let value = authentication::retry(
                    &self.client, self.client.next_id(), "read", self.params,
                    error, panel, interactive, cx,
                ).await?;
                serde_json::from_value(value).map_err(decode_error)
            }
            result => result,
        }
    }
}

#[derive(Default)]
pub(super) struct Requests(BTreeMap<(PathBuf, String, Option<String>), WeakShared<HistoryTask>>);

impl Requests {
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    pub(super) fn invalidate(&mut self, root: &Path) {
        self.0.retain(|(path, _, _), _| path != root);
    }

    fn share(
        &mut self,
        root: &Path,
        path: &str,
        before: Option<&str>,
        start: impl FnOnce() -> HistoryTask,
    ) -> Shared<HistoryTask> {
        let key = (root.to_owned(), path.to_owned(), before.map(str::to_owned));
        if let Some(request) = self.0.get(&key).and_then(WeakShared::upgrade) {
            return request;
        }
        self.0.retain(|_, request| request.upgrade().is_some());
        let request = start().shared();
        if let Some(weak) = request.downgrade() {
            self.0.insert(key, weak);
        }
        request
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_matching_in_flight_history_until_invalidated() {
        let mut requests = Requests::default();
        let root = Path::new("/checkout");
        let start = || {
            Task::ready(Ok(Page {
                revisions: Vec::new(),
                next_before: None,
            }))
        };
        let first = requests.share(root, ".", None, start);
        let same = requests.share(root, ".", None, start);
        let another_page = requests.share(root, ".", Some("1"), start);
        let another_path = requests.share(root, "file.txt", None, start);
        assert!(first.ptr_eq(&same));
        assert!(!first.ptr_eq(&another_page));
        assert!(!first.ptr_eq(&another_path));

        requests.invalidate(root);
        let refreshed = requests.share(root, ".", None, start);
        assert!(!first.ptr_eq(&refreshed));
    }

    #[test]
    fn switching_working_copies_reuses_history_and_pagination() {
        let first_root = Path::new("first");
        let second_root = Path::new("second");
        let mut panel = PanelHistory::default();
        let first = panel.repositories.entry(first_root.into()).or_default();
        for (revision, append) in [(9, false), (8, true)] {
            first.receive(
                Page {
                    revisions: vec![Revision {
                        revision,
                        author: String::new(),
                        date: String::new(),
                        message: String::new(),
                        paths: Vec::new(),
                    }],
                    next_before: Some(revision - 1),
                },
                append,
            );
        }
        first.selected = Some(8);
        panel.filtered = Some(("file.txt".into(), History::default()));
        panel.reset_view(Some(first_root));

        assert!(panel.active(second_root).is_none());
        let first = panel
            .active(first_root)
            .expect("cached working copy history");
        assert_eq!(first.entries.len(), 2);
        assert_eq!(first.next_page(), Some(7));
        assert_eq!(first.selected, None);
        assert!(!first.needs_load(true));
    }

    #[test]
    fn reuses_a_successful_empty_result_until_invalidated() {
        let mut history = History::default();
        assert!(history.needs_load(false));
        history.loading = Some(Task::ready(()));
        assert!(!history.needs_load(true));
        history.loading = None;
        history.failed = true;
        assert!(!history.needs_load(false));
        assert!(history.needs_load(true));
        history.receive(
            Page {
                revisions: Vec::new(),
                next_before: None,
            },
            false,
        );
        assert!(!history.needs_load(true));
        history.invalidate();
        assert!(history.needs_load(false));
    }
}

impl History {
    fn invalidate(&mut self) {
        self.loaded = false;
        self.failed = false;
        self.loading = None;
    }

    fn needs_load(&self, retry_failed: bool) -> bool {
        !self.loaded && self.loading.is_none() && (!self.failed || retry_failed)
    }

    fn next_page(&self) -> Option<u64> {
        self.next_before
            .filter(|_| self.loaded && self.loading.is_none())
    }

    fn receive(&mut self, page: Page, append: bool) {
        if append {
            self.entries.extend(page.revisions);
        } else {
            self.entries = page.revisions;
            self.selected = self
                .selected
                .filter(|selected| self.entries.iter().any(|entry| entry.revision == *selected));
        }
        self.next_before = page.next_before;
        self.loaded = true;
        self.failed = false;
    }
}

impl SvnPanel {
    pub(super) fn preload_history(
        &mut self,
        retry_failed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.selected_root.as_deref() else {
            return;
        };
        let interactive = retry_failed && self.history.visible;
        if let Some(history) = self.history.repositories.get_mut(root) {
            history.interactive |= interactive;
            if !history.needs_load(retry_failed) {
                return;
            }
        }
        self.load_panel_history(None, None, false, interactive, window, cx);
    }

    pub(super) fn load_history(
        &mut self,
        append: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = if append {
            let Some(history) = self
                .selected_root
                .as_deref()
                .and_then(|root| self.history.active(root))
            else {
                return;
            };
            let Some(revision) = history.next_page() else {
                return;
            };
            Some(revision.to_string())
        } else {
            None
        };
        let path = self.history.filtered.as_ref().map(|(path, _)| path.clone());
        self.load_panel_history(path, before.as_deref(), append, true, window, cx);
    }

    pub(super) fn open_filtered_history(
        &mut self,
        path: String,
        before: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.history.filtered = Some((path.clone(), History::default()));
        self.history.visible = true;
        self.load_panel_history(Some(path), before, false, true, window, cx);
    }

    fn load_panel_history(
        &mut self,
        path: Option<String>,
        before: Option<&str>,
        append: bool,
        interactive: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.selected_root.clone() else {
            return;
        };
        let Some(fetch) = self.request_history(&root, path.as_deref().unwrap_or("."), before, cx)
        else {
            cx.notify();
            return;
        };
        let filtered = path.is_some();
        let history = if filtered {
            let Some(history) = self.history.state_mut(&root, true) else {
                return;
            };
            history
        } else {
            self.history.repositories.entry(root.clone()).or_default()
        };
        history.failed = false;
        history.interactive = interactive;
        history.loading = Some(cx.spawn_in(window, async move |this, cx| {
            let result = fetch.load(&this, |panel| {
                panel.selected_root.as_ref() == Some(&root)
                    && panel.history.visible
                    && filtered == panel.history.filtered.is_some()
                    && panel.history.active(&root).is_some_and(|history| history.interactive)
            }, cx).await;
            this.update_in(cx, |this, _, cx| {
                let selected = this.selected_root.as_ref() == Some(&root);
                if filtered && !selected {
                    return;
                }
                let active =
                    selected && this.history.visible && filtered == this.history.filtered.is_some();
                let Some(history) = this.history.state_mut(&root, filtered) else {
                    return;
                };
                history.loading = None;
                match result {
                    Ok(page) => history.receive(page, append),
                    Err(error) => {
                        history.failed = true;
                        if active && error.kind != "cancelled"
                            && (error.kind != "authentication" || history.interactive)
                        {
                            let url = this.repo().map(|repo| repo.copy.url.clone());
                            this.set_error(error, url, cx);
                        }
                    }
                }
                if active {
                    cx.notify();
                }
            })
            .log_err();
        }));
        cx.notify();
    }

    fn request_history(
        &mut self,
        root: &Path,
        path: &str,
        before: Option<&str>,
        cx: &App,
    ) -> Option<HistoryFetch> {
        let client = self.client.clone()?;
        let before = before.filter(|revision| *revision != "HEAD");
        let params = self.repository_params(
            root,
            json!({"operation":"history", "path":path, "before":before}),
        );
        let task = self.history_requests.share(root, path, before, || {
            let client = client.clone();
            let params = params.clone();
            cx.background_spawn(async move {
                let value = client.request(client.next_id(), "read", params).await?;
                serde_json::from_value(value).map_err(decode_error)
            })
        });
        Some(HistoryFetch { client, params, task })
    }

    pub(super) fn discard_history_for(&mut self, root: &Path) {
        self.history_requests.invalidate(root);
        self.history.repositories.remove(root);
        if self.selected_root.as_deref() == Some(root) {
            self.history.filtered = None;
            if self.history.visible {
                self.scroll.set_offset(gpui::point(px(0.), px(0.)));
            }
        }
    }

    pub(super) fn invalidate_repository_history(&mut self, root: &Path) {
        self.history_requests.invalidate(root);
        if let Some(history) = self.history.repositories.get_mut(root) {
            history.invalidate();
        }
    }

    pub(super) fn reconcile_repository_history(&mut self, repos: &[Repo]) {
        self.history.repositories.retain(|root, _| {
            let present = repos.iter().any(|repo| &repo.copy.root == root);
            if !present {
                self.history_requests.invalidate(root);
            }
            present
        });
        for repo in repos {
            self.reconcile_working_copy_history(&repo.copy);
        }
    }

    pub(super) fn reconcile_working_copy_history(&mut self, copy: &Copy) {
        let Some(previous) = self.repos.iter().find(|repo| repo.copy.root == copy.root) else {
            return;
        };
        if copy.url != previous.copy.url || copy.repository_url != previous.copy.repository_url {
            self.discard_history_for(&copy.root);
        } else if copy.revision != previous.copy.revision {
            self.invalidate_repository_history(&copy.root);
        }
    }

    pub(super) fn invalidate_history_after_write(
        &mut self,
        root: &Path,
        switched: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The following status scan preloads the repository history using the updated working copy.
        if switched {
            self.discard_history_for(root);
        } else {
            self.invalidate_repository_history(root);
            if self.selected_root.as_deref() == Some(root)
                && self.history.visible
                && self.history.filtered.is_some()
            {
                self.load_history(false, window, cx);
            }
        }
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let logs = workspace
            .read(cx)
            .items_of_type::<LogView>(cx)
            .filter_map(|log| {
                let view = log.read(cx);
                (view.root == root).then(|| (log.clone(), view.path.clone()))
            })
            .collect::<Vec<_>>();
        for (log, path) in logs {
            if let Some(task) = self.request_history(root, &path, None, cx) {
                log.update(cx, |log, cx| log.load(task, false, false, window, cx));
            }
        }
    }

    pub(super) fn open_log(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(root), Some(workspace)) = (self.selected_root.clone(), self.workspace.upgrade())
        else {
            return;
        };
        let path = path.to_owned();
        let panel = cx.weak_entity();
        let name = root
            .file_name()
            .unwrap_or(root.as_os_str())
            .to_string_lossy()
            .into_owned();
        workspace.update(cx, |workspace, cx| {
            let existing = workspace.items_of_type::<LogView>(cx).find(|log| {
                let log = log.read(cx);
                log.root == root && log.path == path
            });
            if let Some(existing) = existing {
                workspace.activate_item(&existing, true, true, window, cx);
                return;
            }
            let Some(task) = self.request_history(&root, &path, None, cx) else {
                return;
            };
            let log = cx.new(|cx| LogView::new(panel, root, path, name, window, cx));
            log.update(cx, |log, cx| log.load(task, false, true, window, cx));
            let search = log.read(cx).search.focus_handle(cx);
            workspace.add_item_to_active_pane(Box::new(log), None, true, window, cx);
            search.focus(window, cx);
        });
    }

    fn open_revision(
        &mut self,
        root: PathBuf,
        revision: u64,
        target: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled || self.busy || self.client.is_none() {
            return;
        }
        let target = target.map(|path| path.trim_start_matches('/').to_owned());
        if let Some(complete) = preview::reveal_revision(
            &self.workspace,
            &root,
            revision,
            target.as_deref(),
            window,
            cx,
        ) {
            if !matches!(
                &self.preview_request,
                Some(PreviewRequest::Revision { root: pending_root, revision: pending_revision, focused: false, .. })
                    if *pending_root == root && *pending_revision == revision
            ) {
                self.cancel_preview_request();
                if !complete {
                    self.request_revision_review(root, revision, None, false, true, window, cx);
                }
            }
            return;
        }
        if let Some(PreviewRequest::Revision {
            root: pending_root,
            revision: pending_revision,
            target: pending_target,
            focused,
            background,
            ..
        }) = &mut self.preview_request
            && *pending_root == root
            && *pending_revision == revision
        {
            if !*focused && target.is_none() {
                *pending_target = target;
                *background = false;
                return;
            }
            if *focused && *pending_target == target {
                return;
            }
        }
        self.request_revision_review(root, revision, target, true, false, window, cx);
    }

    pub(super) fn request_revision_review(
        &mut self,
        root: PathBuf,
        revision: u64,
        target: Option<String>,
        focus_file: bool,
        background: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let url = self
            .repos
            .iter()
            .find(|repo| repo.copy.root == root)
            .map(|repo| repo.copy.url.clone());
        let params = self.repository_params(
            &root,
            json!({
                "operation":"revision_review",
                "revision":revision,
                "include_files":true,
                "path":target,
                "focus_path":target.as_ref().filter(|_| focus_file),
                "background":background,
            }),
        );
        self.request_preview(
            client,
            Some(root),
            params,
            Output::RevisionReview { revision },
            url,
            window,
            cx,
        );
    }

    pub(super) fn history_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let border = colors.border;
        let hover = colors.element_hover;
        let inactive = colors.editor_background.opacity(0.6);
        let count = self.repo().map_or(0, Repo::change_count);
        let tab = |history: bool, name: SharedString| {
            let active = self.history.visible == history;
            h_flex()
                .id(if history {
                    "svn-history-tab"
                } else {
                    "svn-changes-tab"
                })
                .h_full()
                .flex_1()
                .gap_1()
                .min_w_0()
                .justify_center()
                .cursor_pointer()
                .border_b_1()
                .when(!active, |tab| {
                    tab.bg(inactive).border_color(border.opacity(0.6))
                })
                .hover(move |style| style.bg(hover))
                .child(Label::new(name).color(if active { Color::Default } else { Color::Muted }))
                .when(!history && count > 0, |tab| {
                    tab.child(
                        Label::new(format!("({count})"))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.busy
                        || (this.history.visible == history
                            && this.history.filtered.is_none())
                    {
                        return;
                    }
                    this.scroll.set_offset(gpui::point(px(0.), px(0.)));
                    this.history.reset_view(this.selected_root.as_deref());
                    this.history.visible = history;
                    if history {
                        this.preload_history(true, window, cx);
                    }
                    cx.notify();
                }))
        };
        h_flex()
            .h(ui::Tab::container_height(cx))
            .w_full()
            .child(tab(false, t!("Changes")))
            .child(
                ui::Divider::vertical()
                    .color(ui::DividerColor::BorderFaded)
                    .h_full(),
            )
            .child(tab(true, t!("History")))
            .into_any_element()
    }

    pub(super) fn render_history(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let hover = colors.element_hover;
        let selected = colors.element_selected;
        let focus = colors.panel_focused_border;
        let panel = v_flex().flex_none().min_w_0();
        let graph = self.view_options.history_graph;
        let accent = colors.text_accent;
        let background = colors.panel_background;
        let history = self
            .selected_root
            .as_deref()
            .and_then(|root| self.history.active(root));
        let Some(history) = history.filter(|history| !history.entries.is_empty()) else {
            let loading = history.is_some_and(|history| history.loading.is_some())
                || (self.scanning && history.is_none_or(|history| history.needs_load(false)));
            let message = if loading {
                t!("Loading Commit History…")
            } else if history.is_some_and(|history| history.failed) {
                t!("Failed to load commit history")
            } else {
                t!("No commits yet")
            };
            return panel_ui::panel_placeholder(message, loading).into_any_element();
        };
        let now = time::OffsetDateTime::now_utc();
        let dot = || {
            Label::new("•")
                .size(LabelSize::Small)
                .color(Color::Muted)
                .alpha(0.5)
                .flex_none()
        };
        panel
            .when_some(
                self.history
                    .filtered
                    .as_ref()
                    .filter(|(path, _)| path != "."),
                |panel, (path, _)| {
                    panel.child(
                        div().px_2().py_1().child(
                            Label::new(path.clone())
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate_middle(),
                        ),
                    )
                },
            )
            .children(
                history
                    .entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| {
                        let revision = entry.revision;
                        let expanded = history.selected == Some(revision);
                        let subject = entry.message.lines().next().unwrap_or_default().to_owned();
                        let age = time::OffsetDateTime::parse(
                            &entry.date,
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
                        let id = format!("r{revision}");
                        let tooltip = [
                            id.as_str(),
                            entry.author.as_str(),
                            entry.date.as_str(),
                            entry.message.as_str(),
                        ]
                        .join("\n");
                        v_flex()
                            .child(
                                v_flex()
                                    .id(("svn-revision", revision as usize))
                                    .w_full()
                                    .min_w_0()
                                    .px_2()
                                    .when(!graph, |row| row.py_1().gap_0p5())
                                    .cursor_pointer()
                                    .border_1()
                                    .border_color(if expanded && self.focus.is_focused(window) {
                                        focus
                                    } else {
                                        gpui::transparent_black()
                                    })
                                    .hover(move |row| row.bg(hover))
                                    .when(expanded, |row| row.bg(selected))
                                    .when_else(
                                        graph,
                                        |row| {
                                            row.child(
                                                h_flex()
                                                    .h(rems(1.75))
                                                    .w_full()
                                                    .gap_1p5()
                                                    .child(
                                                        div()
                                                            .relative()
                                                            .w(rems(0.9))
                                                            .h_full()
                                                            .flex_none()
                                                            .child(
                                                                div()
                                                                    .absolute()
                                                                    .left(rems(0.4))
                                                                    .top(if index == 0 {
                                                                        rems(0.875)
                                                                    } else {
                                                                        rems(0.)
                                                                    })
                                                                    .bottom(
                                                                        if index + 1
                                                                            == history.entries.len()
                                                                            || expanded
                                                                        {
                                                                            rems(0.875)
                                                                        } else {
                                                                            rems(0.)
                                                                        },
                                                                    )
                                                                    .w(px(1.))
                                                                    .bg(accent.opacity(0.65)),
                                                            )
                                                            .child(
                                                                div()
                                                                    .absolute()
                                                                    .top(rems(0.625))
                                                                    .left(rems(0.18))
                                                                    .size(rems(0.5))
                                                                    .rounded_full()
                                                                    .border_1()
                                                                    .border_color(accent)
                                                                    .bg(background),
                                                            ),
                                                    )
                                                    .child(
                                                        div().flex_1().min_w_0().child(
                                                            Label::new(subject.clone())
                                                                .size(LabelSize::Small)
                                                                .truncate(),
                                                        ),
                                                    )
                                                    .child(
                                                        Label::new(id.clone())
                                                            .size(LabelSize::Small)
                                                            .color(Color::Muted)
                                                            .flex_none(),
                                                    ),
                                            )
                                        },
                                        |row| {
                                            row.child(Label::new(subject.clone()).truncate()).child(
                                                h_flex()
                                                    .w_full()
                                                    .min_w_0()
                                                    .gap_1p5()
                                                    .child(
                                                        Icon::new(IconName::Person)
                                                            .size(IconSize::Small)
                                                            .color(Color::Muted),
                                                    )
                                                    .when(!entry.author.is_empty(), |row| {
                                                        row.child(
                                                            Label::new(entry.author.clone())
                                                                .size(LabelSize::Small)
                                                                .color(Color::Muted)
                                                                .truncate(),
                                                        )
                                                        .child(dot())
                                                    })
                                                    .when(!age.is_empty(), |row| {
                                                        row.child(
                                                            Label::new(age)
                                                                .size(LabelSize::Small)
                                                                .color(Color::Muted)
                                                                .flex_none(),
                                                        )
                                                        .child(dot())
                                                    })
                                                    .child(
                                                        Label::new(id.clone())
                                                            .size(LabelSize::Small)
                                                            .color(Color::Muted)
                                                            .flex_none(),
                                                    ),
                                            )
                                        },
                                    )
                                    .tooltip(Tooltip::text(tooltip))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.focus.focus(window, cx);
                                        let filtered = this.history.filtered.is_some();
                                        if let Some(root) = this.selected_root.as_deref()
                                            && let Some(history) =
                                                this.history.state_mut(root, filtered)
                                        {
                                            history.selected = (!expanded).then_some(revision);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .when(expanded, |row| {
                                row.child(v_flex().min_w_0().children(
                                    entry.paths.iter().enumerate().map(|(index, changed)| {
                                        let path = &changed.path;
                                        let action = &changed.action;
                                        let diffable = changed.kind != "dir"
                                            && matches!(action.as_str(), "A" | "D" | "M" | "R");
                                        h_flex()
                                            .id(("svn-history-change", index))
                                            .h(rems(1.75))
                                            .w_full()
                                            .min_w_0()
                                            .gap_1p5()
                                            .pl_6()
                                            .pr_2()
                                            .when(diffable && !self.busy, |row| {
                                                row.cursor_pointer()
                                                    .hover(move |row| row.bg(hover))
                                                    .tooltip(Tooltip::text(path.clone()))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        |_, window, _| window.prevent_default(),
                                                    )
                                                    .on_click(cx.listener({
                                                        let path = path.clone();
                                                        move |this, _, window, cx| {
                                                            if let Some(root) =
                                                                this.selected_root.clone()
                                                            {
                                                                this.open_revision(
                                                                    root,
                                                                    revision,
                                                                    Some(path.clone()),
                                                                    window,
                                                                    cx,
                                                                );
                                                            }
                                                        }
                                                    }))
                                            })
                                            .child(if changed.kind == "dir" {
                                                Icon::new(IconName::Folder)
                                                    .size(IconSize::Small)
                                                    .color(Color::Muted)
                                            } else {
                                                panel_ui::status_icon(action)
                                            })
                                            .child(panel_ui::path_label(path, action == "D"))
                                    }),
                                ))
                            })
                    }),
            )
            .when(history.next_before.is_some(), |panel| {
                panel.child(
                    div().px_2().py_2().child(
                        Button::new("svn-more-history", t!("Next"))
                            .disabled(self.busy || history.next_page().is_none())
                            .end_icon(Icon::new(IconName::ChevronDown).size(IconSize::Small))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.load_history(true, window, cx)
                            })),
                    ),
                )
            })
            .into_any_element()
    }
}

struct LogView {
    panel: WeakEntity<SvnPanel>,
    root: PathBuf,
    path: String,
    title: SharedString,
    focus: FocusHandle,
    nav_history: Option<ItemNavHistory>,
    history: History,
    search: Entity<Editor>,
    query: String,
    case_sensitive: bool,
    error: Option<String>,
    table: Entity<TableInteractionState>,
    columns: Entity<RedistributableColumnsState>,
    details_scroll: ScrollHandle,
    tree_view: bool,
    collapsed_paths: BTreeSet<String>,
    stats: BTreeMap<u64, Option<DiffStat>>,
    stats_error: Option<String>,
    stats_request: Option<(Arc<rpc::Client>, u64, Task<()>)>,
    _search_event: Subscription,
    _columns_observation: Subscription,
}

impl LogView {
    fn new(
        panel: WeakEntity<SvnPanel>,
        root: PathBuf,
        path: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            let mut search = Editor::single_line(window, cx);
            search.set_placeholder_text(t!("Search SVN revisions…").as_ref(), window, cx);
            search
        });
        let subscription = cx.observe(&search, |this, search, cx| {
            let query = search.read(cx).text(cx).trim().to_owned();
            if this.query != query {
                this.query = query;
                cx.notify();
            }
        });
        let table = cx.new(|cx| TableInteractionState::new(cx));
        let columns = cx.new(|_| {
            RedistributableColumnsState::new(
                4,
                [0.59, 0.18, 0.14, 0.09]
                    .map(DefiniteLength::Fraction)
                    .to_vec(),
                vec![TableResizeBehavior::Resizable; 4],
            )
        });
        let columns_observation = cx.observe(&columns, |_, _, cx| cx.notify());
        let title = if path == "." {
            format!("{} · {name}", t!("SVN Log"))
        } else {
            format!("{} · {path}", t!("SVN Log"))
        };
        Self {
            panel,
            root,
            path,
            title: title.into(),
            focus: cx.focus_handle(),
            nav_history: None,
            history: History::default(),
            search,
            query: String::new(),
            case_sensitive: false,
            error: None,
            table,
            columns,
            details_scroll: ScrollHandle::new(),
            tree_view: true,
            collapsed_paths: BTreeSet::new(),
            stats: BTreeMap::new(),
            stats_error: None,
            stats_request: None,
            _search_event: subscription,
            _columns_observation: columns_observation,
        }
    }

    fn request(&mut self, append: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.loading.is_some() {
            return;
        }
        let before = append
            .then_some(self.history.next_before)
            .flatten()
            .map(|revision| revision.to_string());
        let Ok(Some(task)) = self.panel.update(cx, |panel, cx| {
            panel.request_history(&self.root, &self.path, before.as_deref(), cx)
        }) else {
            return;
        };
        self.load(task, append, true, window, cx);
    }

    fn load(
        &mut self,
        fetch: HistoryFetch,
        append: bool,
        interactive: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.error = None;
        let panel = self.panel.clone();
        self.history.loading = Some(cx.spawn_in(window, async move |this, cx| {
            let result = fetch.load(&panel, |_| interactive, cx).await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.history.loading = None;
                match result {
                    Ok(page) => this.history.receive(page, append),
                    Err(error) if error.kind == "cancelled" => {}
                    Err(error) if this.history.entries.is_empty() => {
                        this.error = Some(error.message)
                    }
                    Err(error) => {
                        let _ = this.panel.update(cx, |panel, cx| {
                            panel.show_output_status(
                                t!("Failed to load commit history"),
                                error.message,
                                true,
                                cx,
                            );
                        });
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn matching_rows(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return Vec::new();
        }
        let query = if self.case_sensitive {
            self.query.clone()
        } else {
            self.query.to_lowercase()
        };
        let contains = |text: &str| {
            if self.case_sensitive {
                text.contains(&query)
            } else {
                text.to_lowercase().contains(&query)
            }
        };
        self.history
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                (contains(&format!("r{}", entry.revision))
                    || contains(&entry.message)
                    || contains(&entry.author)
                    || entry.paths.iter().any(|path| contains(&path.path)))
                .then_some(index)
            })
            .collect()
    }

    fn selected_row(&self) -> Option<usize> {
        let selected = self.history.selected?;
        self.history
            .entries
            .iter()
            .position(|entry| entry.revision == selected)
    }

    fn select_row(
        &mut self,
        index: usize,
        scroll: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.history.entries.get(index) else {
            return;
        };
        if self.history.selected != Some(entry.revision) {
            self.history.selected = Some(entry.revision);
            self.collapsed_paths.clear();
            self.details_scroll.set_offset(gpui::point(px(0.), px(0.)));
        }
        self.load_stats(window, cx);
        if scroll {
            self.table
                .read(cx)
                .scroll_handle
                .scroll_to_item(index, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn move_match(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let matches = self.matching_rows();
        let Some((first, last)) = matches.first().zip(matches.last()) else {
            return;
        };
        let selected = self.selected_row();
        let next = if forward {
            matches
                .iter()
                .find(|&&row| selected.is_none_or(|selected| row > selected))
                .unwrap_or(first)
        } else {
            matches
                .iter()
                .rev()
                .find(|&&row| selected.is_none_or(|selected| row < selected))
                .unwrap_or(last)
        };
        self.select_row(*next, true, window, cx);
    }

    fn cancel_stats(&mut self) {
        if let Some((client, id, _)) = self.stats_request.take() {
            client.cancel(id);
        }
    }

    fn close_details(&mut self, cx: &mut Context<Self>) {
        self.history.selected = None;
        self.cancel_stats();
        cx.notify();
    }

    fn load_stats(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(revision) = self.history.selected else {
            return;
        };
        self.cancel_stats();
        self.stats_error = None;
        if self.stats.contains_key(&revision) {
            return;
        }
        let Ok(Some((client, params))) = self.panel.read_with(cx, |panel, _| {
            let client = panel.client.clone()?;
            let params = panel.repository_params(
                &self.root,
                json!({"operation":"revision_review","revision":revision}),
            );
            Some((client, params))
        }) else {
            return;
        };
        let id = client.next_id();
        let task = cx.background_spawn({
            let client = client.clone();
            async move {
                let result = client.request(id, "read", params).await?;
                serde_json::from_value::<Option<DiffStat>>(result["stats"].clone())
                    .map_err(decode_error)
            }
        });
        self.stats_request = Some((
            client,
            id,
            cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |this, cx| {
                    if this.history.selected != Some(revision)
                        || !this
                            .stats_request
                            .as_ref()
                            .is_some_and(|(_, pending, _)| *pending == id)
                    {
                        return;
                    }
                    this.stats_request = None;
                    match result {
                        Ok(stats) => {
                            this.stats.insert(revision, stats);
                        }
                        Err(error) => this.stats_error = Some(error.message),
                    }
                    cx.notify();
                });
            }),
        ));
    }

    fn open_selected_revision(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(revision) = self.history.selected {
            self.open_commit(revision, None, window, cx);
        }
    }

    fn open_commit(
        &self,
        revision: u64,
        file: Option<String>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let panel = self.panel.clone();
        let root = self.root.clone();
        // Pane activation deactivates the current item, which may be this log.
        window.defer(cx, move |window, cx| {
            let _ = panel.update(cx, |panel, cx| {
                panel.open_revision(root, revision, file, window, cx);
            });
        });
    }

    fn revision_menu(
        revision: u64,
        message: String,
        root: PathBuf,
        panel: WeakEntity<SvnPanel>,
        available: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        let number = format!("r{revision}");
        ContextMenu::build(window, cx, |menu, _, _| {
            menu.item(
                ui::ContextMenuEntry::new(t!("View Commit"))
                    .disabled(!available)
                    .handler(move |window, cx| {
                        let _ = panel.update(cx, |panel, cx| {
                            panel.open_revision(root.clone(), revision, None, window, cx);
                        });
                    }),
            )
            .separator()
            .entry(format!("{} {number}", t!("Copy")), None, move |_, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(number.clone()));
            })
            .item(
                ui::ContextMenuEntry::new(format!("{} {}", t!("Copy"), t!("Commit Message")))
                    .disabled(message.is_empty())
                    .handler(move |_, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(message.clone()));
                    }),
            )
        })
    }

    fn can_open_revision(&self, cx: &App) -> bool {
        self.panel
            .read_with(cx, |panel, _| !panel.busy && panel.client.is_some())
            .unwrap_or(false)
    }

    fn key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let search_focused = self.search.focus_handle(cx).is_focused(window);
        match event.keystroke.key.as_ref() {
            "enter" if search_focused && !self.query.is_empty() => {
                self.move_match(!event.keystroke.modifiers.shift, window, cx);
            }
            "enter" if !search_focused => self.open_selected_revision(window, cx),
            "escape" if self.history.selected.is_some() => {
                self.close_details(cx);
            }
            key @ ("up" | "down" | "home" | "end") if !search_focused => {
                let Some(last) = self.history.entries.len().checked_sub(1) else {
                    return;
                };
                let index = match (key, self.selected_row()) {
                    ("up", Some(index)) => index.saturating_sub(1),
                    ("down", Some(index)) => (index + 1).min(last),
                    ("end" | "up", None) | ("end", Some(_)) => last,
                    _ => 0,
                };
                self.select_row(index, true, window, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn details(&self, entry: &Revision, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let revision = entry.revision;
        let number = format!("r{revision}");
        let colors = cx.theme().colors();
        let border = colors.border_variant;
        let author_background = colors.element_background;
        let files = entry.paths.iter().filter(|path| path.kind != "dir").count();
        v_flex()
            .min_w_0()
            .size_full()
            .border_l_1()
            .border_color(border)
            .child(
                v_flex()
                    .relative()
                    .flex_none()
                    .w_full()
                    .p_2()
                    .gap_2()
                    .child(
                        div().absolute().top_2().right_2().child(
                            IconButton::new("svn-log-close-details", IconName::Close)
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text(t!("Close")))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_details(cx);
                                })),
                        ),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .py_1()
                            .items_center()
                            .child(
                                h_flex()
                                    .size(rems(2.25))
                                    .rounded_full()
                                    .justify_center()
                                    .bg(author_background)
                                    .child(
                                        Icon::new(IconName::Person)
                                            .size(IconSize::Small)
                                            .color(Color::Muted),
                                    ),
                            )
                            .child(Label::new(entry.author.clone()).mt_1p5().truncate())
                            .child(
                                Label::new(revision_date(entry, cx))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(
                        h_flex().child(
                            Button::new("svn-log-copy-revision", number.clone())
                                .start_icon(
                                    Icon::new(IconName::Hash)
                                        .size(IconSize::Small)
                                        .color(Color::Muted),
                                )
                                .label_size(LabelSize::Small)
                                .color(Color::Muted)
                                .tooltip(Tooltip::text(t!("Copy")))
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        number.clone(),
                                    ));
                                }),
                        ),
                    ),
            )
            .child(ui::Divider::horizontal())
            .child(
                div()
                    .id("svn-log-message")
                    .w_full()
                    .max_h(rems(12.))
                    .overflow_y_scroll()
                    .p_2()
                    .child(Label::new(entry.message.clone())),
            )
            .child(ui::Divider::horizontal())
            .child(
                h_flex()
                    .flex_none()
                    .p_2()
                    .pr_3()
                    .pb_1()
                    .gap_1()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Label::new(if files == 1 {
                                    t!("1 Changed File")
                                } else {
                                    t!("{count} Changed Files", count = files)
                                })
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                            )
                            .child(ui::Divider::vertical())
                            .child(
                                IconButton::new("svn-log-toggle-files-view", IconName::ListTree)
                                    .icon_size(IconSize::Small)
                                    .toggle_state(self.tree_view)
                                    .tooltip(Tooltip::text(if self.tree_view {
                                        t!("Show Flat View")
                                    } else {
                                        t!("Show Tree View")
                                    }))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.tree_view = !this.tree_view;
                                        this.details_scroll.set_offset(gpui::point(px(0.), px(0.)));
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when_some(
                        self.stats.get(&revision).copied().flatten(),
                        |row, stats| {
                            row.child(ui::DiffStat::new(
                                "svn-revision-stat",
                                stats.added,
                                stats.deleted,
                            ))
                        },
                    )
                    .when(self.stats_request.is_some(), |row| {
                        row.child(
                            Icon::new(IconName::LoadCircle)
                                .size(IconSize::Small)
                                .color(Color::Muted)
                                .with_rotate_animation(2),
                        )
                    })
                    .when(
                        self.stats.contains_key(&revision) && self.stats[&revision].is_none(),
                        |row| row.child(Label::new("—").size(LabelSize::Small).color(Color::Muted)),
                    )
                    .when_some(self.stats_error.clone(), |row, error| {
                        row.child(
                            IconButton::new("svn-log-retry-stats", IconName::RotateCw)
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text(error))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.load_stats(window, cx)),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .id("svn-log-changed-files")
                            .size_full()
                            .min_h_0()
                            .track_scroll(&self.details_scroll)
                            .overflow_y_scroll()
                            .children(self.changed_paths(entry, cx)),
                    )
                    .vertical_scrollbar_for(&self.details_scroll, window, cx),
            )
            .child(ui::Divider::horizontal())
            .child(
                h_flex().p_1p5().w_full().child(
                    Button::new("svn-log-view-commit", t!("View Commit"))
                        .full_width()
                        .start_icon(
                            Icon::new(IconName::GitCommit)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .style(ButtonStyle::OutlinedGhost)
                        .disabled(!self.can_open_revision(cx))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_selected_revision(window, cx)
                        })),
                ),
            )
            .into_any_element()
    }

    fn changed_paths(&self, entry: &Revision, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        if self.tree_view {
            let mut tree = LogDirectory::default();
            for change in &entry.paths {
                tree.insert(change);
            }
            self.render_directory(&tree, "", 0, entry.revision, &mut rows, cx);
        } else {
            rows.extend(
                entry
                    .paths
                    .iter()
                    .map(|path| self.changed_path_row(path, 0, true, entry.revision, cx)),
            );
        }
        rows
    }

    fn render_directory(
        &self,
        directory: &LogDirectory<'_>,
        prefix: &str,
        depth: usize,
        revision: u64,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        for (name, directory) in &directory.children {
            let path = format!("{prefix}/{name}");
            let expanded = !self.collapsed_paths.contains(&path);
            rows.push(
                ui::ListItem::new(format!("svn-log-directory-{path}"))
                    .spacing(ui::ListItemSpacing::Sparse)
                    .indent_level(depth)
                    .indent_step_size(px(20.))
                    .start_slot(
                        Icon::new(if expanded {
                            IconName::FolderOpen
                        } else {
                            IconName::Folder
                        })
                        .size(IconSize::Small)
                        .color(Color::Muted),
                    )
                    .child(
                        Label::new((*name).to_owned())
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                    )
                    .tooltip(Tooltip::text(path.clone()))
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |this, _, _, cx| {
                            if expanded {
                                this.collapsed_paths.insert(path.clone());
                            } else {
                                this.collapsed_paths.remove(&path);
                            }
                            cx.notify();
                        }
                    }))
                    .into_any_element(),
            );
            if expanded {
                self.render_directory(directory, &path, depth + 1, revision, rows, cx);
            }
        }
        rows.extend(
            directory
                .files
                .values()
                .map(|path| self.changed_path_row(path, depth, false, revision, cx)),
        );
    }

    fn changed_path_row(
        &self,
        change: &ChangedPath,
        depth: usize,
        flat: bool,
        revision: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (parent, name) = change.path.rsplit_once('/').unwrap_or(("", &change.path));
        let directory = change.kind == "dir";
        let deleted = change.action == "D";
        let path = change.path.clone();
        let action = change.action.clone();
        let openable = !directory && matches!(action.as_str(), "A" | "D" | "M" | "R");
        let row = ui::ListItem::new(format!("svn-log-change-{path}"))
            .spacing(ui::ListItemSpacing::Sparse)
            .indent_level(depth)
            .indent_step_size(px(20.))
            .tooltip(Tooltip::text(path.clone()))
            .start_slot(if directory {
                Icon::new(IconName::Folder)
                    .size(IconSize::Small)
                    .color(Color::Muted)
            } else {
                panel_ui::status_icon(&action)
            })
            .child(
                h_flex()
                    .min_w_0()
                    .gap_1p5()
                    .child(
                        Label::new(name.to_owned())
                            .size(LabelSize::Small)
                            .truncate()
                            .when(deleted, Label::strikethrough),
                    )
                    .when(flat && !parent.is_empty(), |row| {
                        row.child(
                            Label::new(parent.to_owned())
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate_middle(),
                        )
                    }),
            )
            .when(openable, |row| {
                let path = path.clone();
                row.on_click(cx.listener(move |this, _, window, cx| {
                    this.open_commit(revision, Some(path.clone()), window, cx);
                }))
            });
        let log = cx.weak_entity();
        right_click_menu(format!("svn-log-change-menu-{path}"))
            .menu(move |window, cx| {
                let copy_path = path.clone();
                let path = path.clone();
                let log = log.clone();
                ContextMenu::build(window, cx, |menu, _, _| {
                    menu.when(openable, |menu| {
                        menu.entry(t!("View Diff"), None, move |window, cx| {
                            let _ = log.update(cx, |log, cx| {
                                log.open_commit(
                                    revision,
                                    Some(path.clone()),
                                    window,
                                    cx,
                                );
                            });
                        })
                    })
                    .entry(t!("Copy Path"), None, move |_, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_path.clone()));
                    })
                })
            })
            .trigger(move |_, _, _| row)
            .into_any_element()
    }

    fn search_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let matches = self.matching_rows();
        let selection = self
            .selected_row()
            .and_then(|index| matches.iter().position(|&row| row == index))
            .map_or(0, |position| position + 1);
        let no_matches = matches.is_empty();
        let search_focus = self.search.focus_handle(cx);
        let loading = self.history.loading.is_some();
        h_flex()
            .w_full()
            .flex_none()
            .p_1p5()
            .gap_1p5()
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .h_8()
                    .flex_1()
                    .min_w_0()
                    .px_1p5()
                    .gap_1()
                    .border_1()
                    .border_color(colors.border_variant)
                    .rounded_md()
                    .bg(colors.toolbar_background)
                    .track_focus(&search_focus)
                    .child(div().flex_1().min_w_0().child(self.search.clone()))
                    .child(
                        IconButton::new("svn-log-case-sensitive", IconName::CaseSensitive)
                            .shape(ui::IconButtonShape::Square)
                            .toggle_state(self.case_sensitive)
                            .tooltip(Tooltip::text(t!("Match Case Sensitivity")))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.case_sensitive = !this.case_sensitive;
                                this.search.focus_handle(cx).focus(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .min_w(rems(8.))
                    .gap_1()
                    .child(
                        IconButton::new("svn-log-previous-match", IconName::ChevronLeft)
                            .icon_size(IconSize::Small)
                            .disabled(no_matches)
                            .tooltip(Tooltip::text(t!("Select Previous Match")))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.move_match(false, window, cx)
                            })),
                    )
                    .child(
                        IconButton::new("svn-log-next-match", IconName::ChevronRight)
                            .icon_size(IconSize::Small)
                            .disabled(no_matches)
                            .tooltip(Tooltip::text(t!("Select Next Match")))
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.move_match(true, window, cx)
                                }),
                            ),
                    )
                    .child(Label::new(format!("{selection}/{}", matches.len())).color(
                        if no_matches {
                            Color::Disabled
                        } else {
                            Color::Muted
                        },
                    )),
            )
            .child(
                IconButton::new("svn-log-refresh", IconName::RotateCw)
                    .icon_size(IconSize::Small)
                    .disabled(loading)
                    .tooltip(Tooltip::text(t!("Refresh")))
                    .on_click(cx.listener(|this, _, window, cx| this.request(false, window, cx))),
            )
    }

    fn table_rows(
        &mut self,
        range: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Vec<AnyElement>> {
        range
            .map(|index| {
                let entry = &self.history.entries[index];
                let selected = self.history.selected == Some(entry.revision);
                let color = if selected {
                    Color::Default
                } else {
                    Color::Muted
                };
                let subject: SharedString = entry
                    .message
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned()
                    .into();
                let ranges = if self.query.is_empty() {
                    Vec::new()
                } else if self.case_sensitive {
                    subject
                        .match_indices(&self.query)
                        .map(|(start, matched)| start..start + matched.len())
                        .collect::<Vec<_>>()
                } else {
                    let lowercase = subject.to_lowercase();
                    let query = self.query.to_lowercase();
                    // Match offsets can be reused only when case conversion preserves the byte positions.
                    if subject
                        .chars()
                        .map(char::len_utf8)
                        .eq(lowercase.chars().map(char::len_utf8))
                    {
                        lowercase
                            .match_indices(&query)
                            .map(|(start, matched)| start..start + matched.len())
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    }
                };
                let cell =
                    |label: String| Label::new(label).color(color).truncate().into_any_element();
                vec![
                    HighlightedLabel::from_ranges(subject, ranges)
                        .color(color)
                        .truncate()
                        .into_any_element(),
                    cell(revision_date(entry, cx)),
                    cell(entry.author.clone()),
                    cell(format!("r{}", entry.revision)),
                ]
            })
            .collect()
    }
}

impl Drop for LogView {
    fn drop(&mut self) {
        self.cancel_stats();
    }
}

#[derive(Default)]
struct LogDirectory<'a> {
    children: BTreeMap<&'a str, LogDirectory<'a>>,
    files: BTreeMap<&'a str, &'a ChangedPath>,
}

impl<'a> LogDirectory<'a> {
    fn insert(&mut self, change: &'a ChangedPath) {
        let path = change.path.trim_matches('/');
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        if name.is_empty() {
            return;
        }
        let mut node = self;
        for part in parent.split('/').filter(|part| !part.is_empty()) {
            node = node.children.entry(part).or_default();
        }
        if change.kind == "dir" {
            node.children.entry(name).or_default();
        } else {
            node.files.insert(name, change);
        }
    }
}

fn revision_date(entry: &Revision, cx: &App) -> String {
    use project::project_settings::ProjectSettings;
    use settings::{GitGraphDateFormat, Settings as _};
    use std::sync::OnceLock;
    use time::{OffsetDateTime, UtcOffset, format_description::BorrowedFormatItem};

    static DEFAULT: OnceLock<Vec<BorrowedFormatItem<'static>>> = OnceLock::new();
    static ISO: OnceLock<Vec<BorrowedFormatItem<'static>>> = OnceLock::new();
    let (format, description) = match ProjectSettings::get_global(cx).git.graph_date_format {
        GitGraphDateFormat::Default => {
            (&DEFAULT, "[day] [month repr:short] [year] [hour]:[minute]")
        }
        GitGraphDateFormat::Iso => (&ISO, "[year]-[month]-[day] [hour]:[minute]:[second]"),
    };
    let format =
        format.get_or_init(|| time::format_description::parse(description).unwrap_or_default());
    OffsetDateTime::parse(&entry.date, &time::format_description::well_known::Rfc3339)
        .ok()
        .and_then(|date| {
            date.to_offset(UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC))
                .format(format)
                .ok()
        })
        .unwrap_or_default()
}

impl EventEmitter<()> for LogView {}

impl Focusable for LogView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Item for LogView {
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
        self.title.clone()
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<Icon> {
        Some(Icon::new(IconName::Subversion))
    }
}

impl Render for LogView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let hover = colors.element_hover;
        let background = colors.editor_background;
        let border = colors.border_variant;
        let selected = if self.focus.contains_focused(window, cx) {
            colors.element_selected
        } else {
            hover
        };
        let loading = self.history.loading.is_some();
        let row_height = window.text_style().line_height_in_pixels(window.rem_size()) + px(4.);
        let scale = window.scale_factor();
        let row_height = (row_height * scale).round() / scale;
        let selected_row = self.selected_row();
        let log = cx.weak_entity();
        let table = Table::new(4)
            .interactable(&self.table)
            .width_config(ColumnWidthConfig::redistributable(self.columns.clone()))
            .hide_row_borders()
            .hide_row_hover()
            .header(
                [t!("Description"), t!("Date"), t!("Author"), t!("Commit")]
                    .into_iter()
                    .map(|label: SharedString| Label::new(label).color(Color::Muted))
                    .collect(),
            )
            .map_row(move |(index, row), _, _| {
                let active = selected_row == Some(index);
                let log = log.clone();
                let menu_log = log.clone();
                let row = row
                    .h(row_height)
                    .cursor_pointer()
                    .when(active, |row| row.bg(selected))
                    .when(!active, |row| {
                        row.hover(move |row| row.bg(hover.opacity(0.6)))
                    })
                    .on_click(move |event, window, cx| {
                        let _ = log.update(cx, |log, cx| {
                            log.focus.focus(window, cx);
                            log.select_row(index, false, window, cx);
                            if event.click_count() > 1 {
                                log.open_selected_revision(window, cx);
                            }
                        });
                    });
                right_click_menu(("svn-log-revision-menu", index))
                    .maybe_menu(move |window, cx| {
                        let log = menu_log.upgrade()?;
                        let (revision, message, root, panel, available) = {
                            let log = log.read(cx);
                            let entry = log.history.entries.get(index)?;
                            (
                                entry.revision,
                                entry.message.clone(),
                                log.root.clone(),
                                log.panel.clone(),
                                log.can_open_revision(cx),
                            )
                        };
                        Some(LogView::revision_menu(
                            revision, message, root, panel, available, window, cx,
                        ))
                    })
                    .trigger(move |menu_open, _, _| row.when(menu_open, |row| row.bg(selected)))
                    .into_any_element()
            })
            .uniform_list(
                "svn-log-revisions",
                self.history.entries.len(),
                cx.processor(Self::table_rows),
            );
        let detail = selected_row.map(|index| {
            div()
                .w(rems(20.))
                .max_w(gpui::relative(0.45))
                .h_full()
                .flex_none()
                .child(self.details(&self.history.entries[index], window, cx))
        });
        v_flex()
            .size_full()
            .bg(background)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .child(self.search_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        v_flex()
                            .relative()
                            .h_full()
                            .min_w_0()
                            .min_h_0()
                            .flex_1()
                            .map(|body| {
                                if self.history.entries.is_empty() {
                                    let message = if let Some(error) = &self.error {
                                        t!("Error loading: {error}", error = error.as_str())
                                    } else if loading {
                                        t!("Loading")
                                    } else {
                                        t!("No commits found")
                                    };
                                    body.child(panel_ui::panel_placeholder(message, loading))
                                } else {
                                    body.child(div().flex_1().min_h_0().child(table))
                                }
                            })
                            .when(self.history.next_before.is_some(), |list| {
                                list.child(
                                    h_flex()
                                        .px_2()
                                        .py_1()
                                        .border_t_1()
                                        .border_color(border)
                                        .child(
                                            Button::new(
                                                "svn-log-more",
                                                t!("Load Earlier Revisions"),
                                            )
                                            .disabled(loading)
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.request(true, window, cx)
                                                }),
                                            ),
                                        ),
                                )
                            }),
                    )
                    .children(detail),
            )
    }
}
