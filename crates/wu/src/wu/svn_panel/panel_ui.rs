use super::*;
use changes_view::{Entry, Row, Section};
use gpui::Anchor;
use std::path::Path;
use ui::{
    ButtonLike, CommonAnimationExt, ContextMenu, ContextMenuEntry, ElevationIndex, ListItemSpacing,
    PopoverMenu, SplitButton, ToggleState, WithScrollbar, right_click_menu,
};

const CHANGE_TREE_INDENT: f32 = 16.0;

#[derive(Clone, Copy)]
enum MenuAction {
    Form(form::Kind),
    Read(&'static str, Output),
    Write(&'static str),
    UpdateAll,
    RemoteStatus,
    Refresh,
    SelectAll(bool),
    ShowOutput,
    FileHistory,
    VerifyCertificate,
}

pub(super) fn panel_placeholder(message: SharedString, loading: bool) -> gpui::Div {
    v_flex()
        .flex_1()
        .w_full()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_1()
        .px_3()
        .child(
            h_flex()
                .gap_1()
                .max_w_full()
                .child(
                    div()
                        .text_center()
                        .child(Label::new(message).color(Color::Muted)),
                )
                .when(loading, |row| {
                    row.child(
                        Icon::new(IconName::LoadCircle)
                            .size(IconSize::Small)
                            .color(Color::Accent)
                            .with_rotate_animation(3),
                    )
                }),
        )
}

pub(super) fn status_icon(code: &str) -> Icon {
    let (icon, color) = match code {
        "C" => (IconName::Warning, Color::VersionControlConflict),
        "A" | "?" => (IconName::SquarePlus, Color::VersionControlAdded),
        "D" | "!" => (IconName::SquareMinus, Color::VersionControlDeleted),
        "M" | "P" | "R" => (IconName::SquareDot, Color::VersionControlModified),
        "X" => (IconName::ArrowUpRight, Color::Muted),
        _ => (IconName::File, Color::Muted),
    };
    Icon::new(icon).size(IconSize::Small).color(color)
}

fn change_path_cell(
    tree: bool,
    depth: usize,
    expanded: Option<bool>,
    icon: Icon,
    label: impl IntoElement,
) -> gpui::Div {
    h_flex()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .gap_1()
        .when(tree, |cell| {
            cell.pl(px(depth as f32 * CHANGE_TREE_INDENT)).child(
                h_flex()
                    .flex_none()
                    .size(IconSize::XSmall.rems())
                    .children(expanded.map(|expanded| {
                        Icon::new(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(IconSize::XSmall)
                        .color(Color::Muted)
                    })),
            )
        })
        .child(h_flex().flex_none().child(icon))
        .child(label)
}

pub(super) fn path_label(path: &str, deleted: bool) -> impl IntoElement {
    let path = Path::new(path);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path.to_str().unwrap_or_default());
    let parent = path
        .parent()
        .and_then(Path::to_str)
        .filter(|part| !part.is_empty());
    h_flex()
        .min_w_0()
        .gap_1p5()
        .child(
            Label::new(name.to_owned())
                .single_line()
                .truncate()
                .when(deleted, Label::strikethrough),
        )
        .when_some(parent, |row, parent| {
            row.child(
                Label::new(parent.to_owned())
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .truncate_middle(),
            )
        })
}

impl SvnPanel {
    fn worktree_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_worktree_diff(None, window, cx);
    }

    pub(super) fn show_worktree_diff(
        &mut self,
        target: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy || self.client.is_none() {
            return;
        }
        let Some(root) = self.selected_root.as_deref() else {
            return;
        };
        if let Some((revealed, complete)) = preview::reveal_worktree(
            &self.workspace,
            root,
            self.revision,
            target.as_deref(),
            window,
            cx,
        ) {
            if target.is_none()
                || !matches!(
                    self.preview_request,
                    Some(PreviewRequest::Worktree { focused: false, .. })
                )
            {
                self.cancel_preview_request();
            }
            if !revealed && let Some(path) = target {
                self.open_path_diff(path, window, cx);
            } else if revealed && !complete && self.preview_request.is_none() {
                self.load_worktree_overview(window, cx);
            }
            return;
        }
        if let Some(PreviewRequest::Worktree {
            target: pending,
            focused,
            background,
            ..
        }) = &mut self.preview_request
        {
            if !*focused && target.is_none() {
                *pending = target;
                *background = false;
                return;
            }
            if *focused && *pending == target {
                return;
            }
        }
        self.request_worktree_comparison(target, false, window, cx);
    }

    pub(super) fn load_worktree_overview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_worktree_comparison(None, true, window, cx);
    }

    fn request_worktree_comparison(
        &mut self,
        target: Option<String>,
        background: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run(
            "read",
            json!({
                "operation":"worktree_diff",
                "focus_path":target,
                "background":background,
            }),
            Output::WorktreeDiff,
            window,
            cx,
        );
    }

    pub(super) fn open_path_diff(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run(
            "read",
            json!({"operation":"diff","path":path}),
            Output::Diff,
            window,
            cx,
        );
    }

    fn action_entry(
        panel: WeakEntity<Self>,
        label: SharedString,
        action: MenuAction,
        disabled: bool,
    ) -> ContextMenuEntry {
        ContextMenuEntry::new(label)
            .disabled(disabled)
            .handler(move |window, cx| {
                let _ = panel.update(cx, |this, cx| {
                    if this.busy {
                        return;
                    }
                    match action {
                        MenuAction::Form(kind) => this.start_form(kind, window, cx),
                        MenuAction::Read(operation, output) => {
                            if matches!(output, Output::WorktreeDiff) {
                                this.worktree_diff(window, cx);
                                return;
                            }
                            let path = this.read_path(cx);
                            this.run(
                                "read",
                                json!({"operation":operation,"path":path}),
                                output,
                                window,
                                cx,
                            );
                        }
                        MenuAction::Write(operation) => this.simple_write(operation, window, cx),
                        MenuAction::UpdateAll => this.confirm_all_updates(window, cx),
                        MenuAction::Refresh => {
                            if this.history.visible {
                                this.load_history(false, window, cx);
                            } else {
                                this.refresh(true, window, cx);
                            }
                        }
                        MenuAction::RemoteStatus => this.run(
                            "status",
                            json!({"remote":true,"ignored":this.view_options.show_ignored,"diff_stats":true}),
                            Output::RemoteStatus,
                            window,
                            cx,
                        ),
                        MenuAction::FileHistory => {
                            let path = this.read_path(cx);
                            this.open_log(&path, window, cx);
                        }
                        MenuAction::ShowOutput => {
                            if let Some(output) = this.last_output.clone() {
                                preview::text(
                                    this.workspace.clone(),
                                    t!("SVN Output"),
                                    output,
                                    window,
                                    cx,
                                );
                            }
                        }
                        MenuAction::SelectAll(checked) => {
                            let paths = this.committable_paths();
                            this.select_paths(paths, checked, cx);
                        }
                        MenuAction::VerifyCertificate => this.certificate(window, cx),
                    }
                });
            })
    }

    fn row_menu(
        panel: WeakEntity<Self>,
        change: Change,
        is_dir: bool,
        absolute: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        use form::Kind;
        let present = !matches!(change.item.as_str(), "missing" | "deleted" | "external");
        let available = present && !is_dir;
        let versioned = !matches!(change.item.as_str(), "ignored" | "unversioned" | "external")
            && !change.file_external;
        let tracked_file = versioned
            && !is_dir
            && !matches!(change.item.as_str(), "added" | "missing" | "deleted");
        let changed = change.committable() || change.conflicted() || change.item == "missing";
        let target = change.path.clone();
        let entry = |label, action, enabled: bool| {
            let panel = panel.clone();
            let path = target.clone();
            ContextMenuEntry::new(label)
                .disabled(!enabled)
                .handler(move |window, cx| {
                    let _ = panel.update(cx, |this, cx| {
                        if this.busy {
                            return;
                        }
                        this.active_directory = None;
                        this.active_path = Some(path.clone());
                        match action {
                            MenuAction::Form(kind) => {
                                this.start_form_for(kind, path.clone(), true, window, cx)
                            }
                            MenuAction::Read(operation, output) => this.run(
                                "read",
                                json!({"operation":operation,"path":path}),
                                output,
                                window,
                                cx,
                            ),
                            MenuAction::Write(operation) => {
                                this.write_paths(operation, vec![path.clone()], window, cx)
                            }
                            MenuAction::FileHistory => {
                                this.open_log(&path, window, cx);
                            }
                            _ => {}
                        }
                        cx.notify();
                    });
                })
        };
        ContextMenu::build(window, cx, |menu, _, cx| {
            let open_panel = panel.clone();
            let open_path = target.clone();
            let select_panel = panel.clone();
            let select_path = target.clone();
            let selected = panel
                .read_with(cx, |this, _| this.selected_paths.contains(&target))
                .unwrap_or(false);
            let conflicted = change.conflicted();
            let (operation, output) = if conflicted {
                ("conflict", Output::Conflict)
            } else {
                ("diff", Output::Diff)
            };
            let menu = menu
                .when(change.committable(), |menu| {
                    menu.item(
                        ContextMenuEntry::new(t!("Include in SVN Commit"))
                            .toggle(ui::IconPosition::End, selected)
                            .handler(move |_, cx| {
                                let _ = select_panel.update(cx, |this, cx| {
                                    this.select_change(select_path.clone(), !selected, cx)
                                });
                            }),
                    )
                })
                .item(
                    ContextMenuEntry::new(t!("Open File"))
                        .disabled(!available)
                        .handler(move |window, cx| {
                            let _ = open_panel.update(cx, |this, cx| {
                                this.open_file(open_path.clone(), window, cx)
                            });
                        }),
                )
                .item(entry(
                    if conflicted {
                        t!("Resolve SVN Conflict")
                    } else {
                        t!("View Diff")
                    },
                    MenuAction::Read(operation, output),
                    change.can_view_diff(),
                ))
                .separator()
                .item(entry(
                    t!("SVN Add"),
                    MenuAction::Write("add"),
                    change.item == "unversioned",
                ))
                .item(entry(
                    t!("SVN Delete"),
                    MenuAction::Write("delete"),
                    versioned && !conflicted && change.item != "deleted" && change.path != ".",
                ))
                .item(entry(
                    Kind::Move.label(),
                    MenuAction::Form(Kind::Move),
                    versioned && present && !conflicted && change.path != ".",
                ))
                .item(entry(
                    if is_dir {
                        t!("Revert Subtree")
                    } else {
                        t!("SVN Revert")
                    },
                    MenuAction::Write(if is_dir { "revert_tree" } else { "revert" }),
                    changed,
                ))
                .separator()
                .item(entry(
                    t!("View File History"),
                    MenuAction::FileHistory,
                    versioned && change.item != "added",
                ))
                .item(entry(
                    Kind::Blame.label(),
                    MenuAction::Form(Kind::Blame),
                    tracked_file,
                ))
                .item(entry(
                    t!("Show SVN Properties"),
                    MenuAction::Read("properties", Output::Text),
                    versioned,
                ))
                .item(entry(
                    Kind::SetProperty.label(),
                    MenuAction::Form(Kind::SetProperty),
                    versioned && present,
                ))
                .item(entry(
                    Kind::RemoveProperty.label(),
                    MenuAction::Form(Kind::RemoveProperty),
                    versioned && present,
                ))
                .item(entry(
                    Kind::Lock.label(),
                    MenuAction::Form(Kind::Lock),
                    tracked_file && !change.locked,
                ))
                .item(entry(
                    t!("SVN Unlock"),
                    MenuAction::Write("unlock"),
                    change.can_unlock(),
                ))
                .item(entry(
                    Kind::Changelist.label(),
                    MenuAction::Form(Kind::Changelist),
                    tracked_file,
                ))
                .item(entry(
                    t!("Remove Changelist"),
                    MenuAction::Write("changelist"),
                    change.changelist.is_some(),
                ))
                .separator();
            Self::copy_paths_menu(menu, &target, &absolute)
        })
    }

    fn actions_menu(&self, split: bool, cx: &mut Context<Self>) -> AnyElement {
        use form::Kind;
        let panel = cx.weak_entity();
        let copy_selected = self.repo().is_some();
        let has_output = self.last_output.is_some();
        let verify_certificate = self.certificate_recovery().is_some();
        let needs_credentials = self.needs_credentials();
        let can_diff = self.worktree_diff_available();
        let active_path = self.active_path.as_ref();
        let conflicted = self.repo().is_some_and(|repo| {
            repo.entries
                .iter()
                .any(|entry| active_path == Some(&entry.path) && entry.conflicted())
        });
        let paths = self.committable_paths();
        let has_selected = paths.iter().any(|path| self.selected_paths.contains(path));
        let can_select = paths.iter().any(|path| !self.selected_paths.contains(path));
        let menu = PopoverMenu::new("svn-actions");
        let menu = if split {
            menu.trigger_with_tooltip(
                ButtonLike::new_rounded_right("svn-actions-trigger")
                    .layer(ElevationIndex::ModalSurface)
                    .size(ButtonSize::Compact)
                    .disabled(self.client.is_none() || self.busy)
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall)),
                Tooltip::text(t!("Actions")),
            )
        } else {
            menu.trigger_with_tooltip(
                IconButton::new("svn-actions-trigger", IconName::Ellipsis)
                    .icon_size(IconSize::Small)
                    .disabled(self.client.is_none() || self.busy),
                Tooltip::text(t!("Actions")),
            )
        };
        menu.anchor(Anchor::TopRight)
            .menu(move |window, cx| {
                let entry = |label, action| (label, action);
                let form = |kind: Kind| (kind.label(), MenuAction::Form(kind));
                Some(ContextMenu::build(window, cx, |menu, _, cx| {
                    let mut menu = menu
                        .item(Self::action_entry(
                            panel.clone(),
                            Kind::Checkout.label(),
                            MenuAction::Form(Kind::Checkout),
                            false,
                        ))
                        .item(Self::action_entry(
                            panel.clone(),
                            t!("Refresh"),
                            MenuAction::Refresh,
                            false,
                        ))
                        .when(verify_certificate, |menu| {
                            menu.item(Self::action_entry(
                                panel.clone(),
                                t!("Verify SVN Server Certificate"),
                                MenuAction::VerifyCertificate,
                                false,
                            ))
                        })
                        .when(needs_credentials, |menu| {
                            menu.item(Self::action_entry(
                                panel.clone(),
                                Kind::Credentials.label(),
                                MenuAction::Form(Kind::Credentials),
                                false,
                            ))
                        });
                    if !copy_selected {
                        return menu;
                    }
                    menu = menu
                        .item(Self::action_entry(
                            panel.clone(),
                            t!("SVN Output"),
                            MenuAction::ShowOutput,
                            !has_output,
                        ))
                        .separator()
                        .item(Self::action_entry(
                            panel.clone(),
                            t!("Select All"),
                            MenuAction::SelectAll(true),
                            !can_select,
                        ))
                        .item(Self::action_entry(
                            panel.clone(),
                            t!("Clear Selection"),
                            MenuAction::SelectAll(false),
                            !has_selected,
                        ));
                    let groups = [
                        (
                            t!("Changes"),
                            vec![
                                entry(
                                    t!("Working Copy Diff"),
                                    MenuAction::Read("worktree_diff", Output::WorktreeDiff),
                                ),
                                entry(t!("SVN Add"), MenuAction::Write("add")),
                                entry(t!("SVN Delete"), MenuAction::Write("delete")),
                                form(Kind::Move),
                                entry(t!("SVN Revert"), MenuAction::Write("revert")),
                                entry(t!("Revert Subtree"), MenuAction::Write("revert_tree")),
                                form(Kind::Changelist),
                                entry(t!("Remove Changelist"), MenuAction::Write("changelist")),
                            ],
                        ),
                        (
                            t!("History"),
                            vec![
                                form(Kind::History),
                                form(Kind::DiffRevision),
                                form(Kind::Blame),
                            ],
                        ),
                        (
                            t!("Repositories"),
                            vec![
                                form(Kind::Update),
                                entry(t!("Update All SVN Working Copies"), MenuAction::UpdateAll),
                                entry(t!("Check Remote Status"), MenuAction::RemoteStatus),
                                form(Kind::Branches),
                                form(Kind::CopyBranch),
                                form(Kind::Switch),
                                form(Kind::Merge),
                            ],
                        ),
                        (
                            t!("General"),
                            vec![
                                entry(
                                    t!("Show SVN Properties"),
                                    MenuAction::Read("properties", Output::Text),
                                ),
                                form(Kind::SetProperty),
                                form(Kind::RemoveProperty),
                                form(Kind::Lock),
                                entry(t!("SVN Unlock"), MenuAction::Write("unlock")),
                                entry(t!("SVN Cleanup"), MenuAction::Write("cleanup")),
                                form(Kind::Credentials),
                            ],
                        ),
                    ];
                    let focused = panel.read_with(cx, |panel, cx| panel.read_path(cx)).ok();
                    let has_selection = panel
                        .read_with(cx, |panel, _| !panel.selected_paths.is_empty())
                        .unwrap_or(false);
                    menu = menu.separator();
                    for (heading, entries) in groups {
                        let panel = panel.clone();
                        let entries = entries
                            .into_iter()
                            .map(|(label, action)| {
                                let disabled = match action {
                                    MenuAction::Read(_, Output::WorktreeDiff) => !can_diff,
                                    MenuAction::Write(operation) if operation != "cleanup" => panel
                                        .read_with(cx, |panel, cx| {
                                            panel.path_operation_targets(operation, cx).is_empty()
                                        })
                                        .unwrap_or(true),
                                    MenuAction::Form(Kind::Move | Kind::Blame) => {
                                        focused.as_deref().is_none_or(|path| path == ".")
                                    }
                                    MenuAction::Form(Kind::Lock | Kind::Changelist) => {
                                        !has_selection
                                            && focused.as_deref().is_none_or(|path| path == ".")
                                    }
                                    _ => false,
                                };
                                (label, action, disabled)
                            })
                            .collect::<Vec<_>>();
                        menu = menu.submenu(heading, move |submenu, _, _| {
                            entries
                                .iter()
                                .fold(submenu, |submenu, (label, action, disabled)| {
                                    submenu.item(Self::action_entry(
                                        panel.clone(),
                                        label.clone(),
                                        *action,
                                        *disabled,
                                    ))
                                })
                        });
                    }
                    menu.separator().item(Self::action_entry(
                        panel.clone(),
                        t!("Resolve SVN Conflict"),
                        MenuAction::Read("conflict", Output::Conflict),
                        !conflicted,
                    ))
                }))
            })
            .into_any_element()
    }

    pub(super) fn committable_paths(&self) -> Vec<String> {
        self.repo()
            .into_iter()
            .flat_map(|repo| &repo.entries)
            .filter(|entry| entry.committable())
            .map(|entry| entry.path.clone())
            .collect()
    }

    pub(super) fn select_paths(
        &mut self,
        paths: Vec<String>,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        let active = self.active_path.clone();
        let directory = self.active_directory.take();
        for path in paths {
            self.select_change(path, checked, cx);
        }
        self.active_path = active;
        self.active_directory = directory;
        cx.notify();
    }

    fn worktree_diff_available(&self) -> bool {
        self.repo().is_some_and(|repo| {
            repo.entries
                .iter()
                .any(|entry| entry.can_view_diff() && !entry.conflicted())
        })
    }

    pub(super) fn diff_totals(&self) -> DiffStat {
        self.repo()
            .into_iter()
            .flat_map(|repo| &repo.entries)
            .filter_map(|entry| entry.diff_stat)
            .fold(DiffStat::default(), |mut total, stat| {
                total.added += stat.added;
                total.deleted += stat.deleted;
                total
            })
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let paths = self.committable_paths();
        let all = !paths.is_empty() && paths.iter().all(|path| self.selected_paths.contains(path));
        let diff_available = self.worktree_diff_available();
        let diff_total = self.diff_totals();
        let ready = self.client.is_some() && !self.busy;
        h_flex()
            .min_h(ui::Tab::container_height(cx))
            .w_full()
            .pl_1()
            .pr_1()
            .flex_none()
            .when(!self.history.visible, |row| row.flex_wrap())
            .gap_1()
            .justify_between()
            .child(if self.history.visible {
                self.repository_switcher(cx).into_any_element()
            } else {
                ButtonLike::new("svn-worktree-diff")
                    .size(ButtonSize::Compact)
                    .disabled(!ready || !diff_available)
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Icon::new(IconName::Diff)
                                    .size(IconSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(
                                Label::new(t!("View Diff"))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .when(diff_total.added + diff_total.deleted > 0, |row| {
                                row.child(ui::DiffStat::new(
                                    "svn-total-diff-stat",
                                    diff_total.added,
                                    diff_total.deleted,
                                ))
                            }),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.worktree_diff(window, cx)))
                    .into_any_element()
            })
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .when(self.history.visible, |tools| {
                        tools
                            .child(
                                IconButton::new("svn-open-log", IconName::GitGraph)
                                    .icon_size(IconSize::Small)
                                    .disabled(!ready)
                                    .tooltip(Tooltip::text(t!("Open SVN Log")))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_log(".", window, cx)
                                    })),
                            )
                            .child(self.history_options_menu(cx))
                    })
                    .when(self.history.visible || self.repo().is_none(), |tools| {
                        tools.child(self.actions_menu(false, cx))
                    })
                    .when(!self.history.visible && self.repo().is_some(), |tools| {
                        tools
                            .child(self.view_options_menu(cx))
                            .child(SplitButton::new(
                                ButtonLike::new_rounded_left("svn-select-all")
                                    .layer(ElevationIndex::ModalSurface)
                                    .size(ButtonSize::Compact)
                                    .disabled(self.busy || paths.is_empty())
                                    .child(
                                        Label::new(if all {
                                            t!("Clear Selection")
                                        } else {
                                            t!("Select All")
                                        })
                                        .size(LabelSize::Small)
                                        .mr_0p5(),
                                    )
                                    .tooltip(Tooltip::text(t!(
                                        "Select changes for the next SVN commit"
                                    )))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_paths(paths.clone(), !all, cx);
                                    })),
                                self.actions_menu(true, cx),
                            ))
                    })
                    .when(self.busy, |tools| tools.child(self.cancel_operation(cx))),
            )
            .into_any_element()
    }

    pub(super) fn cancel_operation(&self, cx: &mut Context<Self>) -> IconButton {
        IconButton::new("svn-cancel", IconName::Close)
            .icon_size(IconSize::Small)
            .disabled(self.cancellation_requested)
            .tooltip(Tooltip::text(if self.cancellation_requested {
                t!("Cancelling SVN operation…")
            } else {
                t!("Cancel")
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                this.cancellation_requested = true;
                if let (Some(client), Some(id)) = (&this.client, this.operation_id) {
                    client.cancel(id);
                }
                cx.notify();
            }))
    }

    fn repository_rows(&self) -> Vec<RepositoryRow> {
        let mut rows = self
            .repos
            .iter()
            .map(RepositoryRow::from)
            .collect::<Vec<_>>();
        rows.sort_by_cached_key(|row| (row.name.to_lowercase(), row.root.clone()));
        for group in
            rows.chunk_by_mut(|left, right| left.name.to_lowercase() == right.name.to_lowercase())
        {
            if group.len() < 2 {
                continue;
            }
            let common = group[0]
                .root
                .ancestors()
                .find(|ancestor| group.iter().all(|row| row.root.starts_with(ancestor)))
                .map(Path::to_path_buf);
            for row in group {
                let path = common
                    .as_ref()
                    .and_then(|ancestor| row.root.strip_prefix(ancestor).ok())
                    .filter(|path| path.components().count() > 0)
                    .unwrap_or(&row.root);
                row.location = Some(path.display().to_string());
            }
        }
        rows
    }

    pub(super) fn reveal_selected_repository(&self) {
        if let Some(index) = self
            .repository_rows()
            .iter()
            .position(|row| Some(&row.root) == self.selected_root.as_ref())
        {
            self.repositories_scroll.scroll_to_item(index);
        }
    }

    fn repository_section(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.repos.len() < 2 {
            return None;
        }
        let colors = cx.theme().colors();
        let border = colors.border_variant;
        let collapsed = self.repositories_collapsed;
        let hover = colors.ghost_element_hover;
        let selection = colors.ghost_element_selected;
        let rows = (!collapsed).then(|| {
            div()
                .relative()
                .max_h(rems(8.75))
                .child(
                    v_flex()
                        .id("svn-repositories-list")
                        .max_h(rems(8.75))
                        .track_scroll(&self.repositories_scroll)
                        .overflow_y_scroll()
                        .children(self.repository_rows().into_iter().map(|repo| {
                            let active = self.selected_root.as_ref() == Some(&repo.root);
                            let tooltip = repo.tooltip();
                            let root = repo.root.clone();
                            h_flex()
                                .id(SharedString::from(format!(
                                    "svn-repository-row-{}",
                                    root.display()
                                )))
                                .h(rems(1.75))
                                .flex_none()
                                .w_full()
                                .pl_5()
                                .pr_2()
                                .gap_1p5()
                                .when(active, |row| row.bg(selection))
                                .hover(move |row| row.bg(hover))
                                .cursor_pointer()
                                .tooltip(Tooltip::text(tooltip))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.focus.focus(window, cx);
                                    this.switch_repository(root.clone(), window, cx);
                                }))
                                .children(repo.panel_labels(active))
                        })),
                )
                .vertical_scrollbar_for(&self.repositories_scroll, window, cx)
        });
        Some(
            v_flex()
                .w_full()
                .flex_none()
                .pb_1()
                .border_b_1()
                .border_color(border)
                .child(
                    h_flex()
                        .id("svn-repositories-header")
                        .h(rems(1.75))
                        .w_full()
                        .pl_2p5()
                        .gap_1()
                        .cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .child(
                            Icon::new(if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                        )
                        .child(
                            Label::new(t!("Repositories"))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.repositories_collapsed = !this.repositories_collapsed;
                            if !this.repositories_collapsed {
                                this.reveal_selected_repository();
                            }
                            cx.notify();
                        })),
                )
                .children(rows)
                .into_any_element(),
        )
    }

    fn repository_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(repo) = self.repo() else {
            return gpui::Empty.into_any_element();
        };
        let copy = repo.copy.clone();
        let rows = self.repository_rows();
        let Some(repo) = rows.iter().find(|row| row.root == copy.root) else {
            return gpui::Empty.into_any_element();
        };
        let multiple = rows.len() > 1;
        let panel = cx.weak_entity();
        let branch_panel = panel.clone();
        let active = repo.root.clone();
        let branch = repo.short_branch();
        let repository_label = repo.location.clone().unwrap_or_else(|| repo.name.clone());
        let repository_tooltip = format!(
            "{}\n{}",
            t!("Switch Active Repository"),
            repo.root.display()
        );
        let branch_tooltip = format!("{}\n{}", t!("Switch Branch"), repo.url);
        let anchor = if self.history.visible {
            Anchor::TopLeft
        } else {
            Anchor::BottomLeft
        };
        let offset = gpui::point(
            px(0.),
            if self.history.visible {
                px(2.)
            } else {
                px(-2.)
            },
        );
        let repo_picker = PopoverMenu::new("svn-repository-picker")
            .trigger_with_tooltip(
                Button::new("svn-repository", repository_label)
                    .size(ButtonSize::None)
                    .label_size(LabelSize::Small)
                    .truncate(true)
                    .disabled(self.busy),
                Tooltip::text(repository_tooltip),
            )
            .anchor(anchor)
            .offset(offset)
            .menu(move |window, cx| {
                Some(cx.new(|cx| {
                    RepositoryPicker::new(rows.clone(), active.clone(), panel.clone(), window, cx)
                }))
            });
        let branch_picker = PopoverMenu::new("svn-branch-picker")
            .trigger_with_tooltip(
                Button::new("svn-branch", branch)
                    .size(ButtonSize::None)
                    .label_size(LabelSize::Small)
                    .truncate(true)
                    .disabled(self.busy || self.client.is_none()),
                Tooltip::text(branch_tooltip),
            )
            .anchor(anchor)
            .offset(offset)
            .menu(move |window, cx| {
                let (client, params) = branch_panel
                    .read_with(cx, |panel, _| {
                        let client = panel.client.clone()?;
                        let params = panel
                            .repository_params(&copy.root, json!({"operation":"switch_targets"}));
                        Some((client, params))
                    })
                    .ok()
                    .flatten()?;
                Some(cx.new(|cx| {
                    branch_picker::BranchPicker::new(
                        branch_panel.clone(),
                        copy.clone(),
                        client,
                        params,
                        window,
                        cx,
                    )
                }))
            });
        h_flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .gap_px()
            .child(
                Icon::new(IconName::Subversion)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .when(multiple, |bar| {
                bar.child(div().min_w_0().child(repo_picker))
                    .child(Label::new("/").size(LabelSize::Small).color(Color::Muted))
            })
            .child(div().min_w_0().child(branch_picker))
            .into_any_element()
    }

    fn render_changes(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(repo) = self.repo() else {
            return gpui::Empty.into_any_element();
        };
        if let Some(error) = &repo.error {
            return panel_placeholder(t!("Error loading: {error}", error = error.as_str()), false)
                .child(self.refresh_button(cx))
                .into_any_element();
        }
        let groups = self.change_groups();
        if groups.is_empty() {
            return panel_placeholder(t!("No changes to commit"), false).into_any_element();
        }
        let root = repo.copy.root.clone();
        v_flex()
            .flex_none()
            .min_w_0()
            .children(groups.into_iter().map(|group| {
                let section = group.section;
                let collapsed = self.collapsed_groups.contains(&section);
                let name = section.test_id();
                v_flex()
                    .when_some(section.label(), |group_row, label| {
                        group_row.child(
                            h_flex()
                                .id(format!("svn-section-{name}"))
                                .h(rems(1.75))
                                .w_full()
                                .pl_2p5()
                                .pr_1()
                                .gap_1p5()
                                .border_1()
                                .border_r_2()
                                .cursor_pointer()
                                .hover(|row| row.bg(cx.theme().colors().ghost_element_hover))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.focus.focus(window, cx);
                                    if !this.collapsed_groups.insert(section) {
                                        this.collapsed_groups.remove(&section);
                                    }
                                    cx.notify();
                                }))
                                .child(
                                    Icon::new(if collapsed {
                                        IconName::ChevronRight
                                    } else {
                                        IconName::ChevronDown
                                    })
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                                )
                                .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
                                .child(div().flex_1())
                                .children(self.selection_checkbox(
                                    format!("svn-section-select-{name}"),
                                    group.paths,
                                    cx,
                                )),
                        )
                    })
                    .children(group.rows.into_iter().map(|row| match row {
                        Entry::Change { change, depth } => {
                            self.render_change_row(change, depth, &root, window, cx)
                        }
                        Entry::Directory {
                            path,
                            name,
                            depth,
                            change,
                            paths,
                            collapsed,
                        } => self.render_directory_row(
                            section, path, name, depth, change, paths, collapsed, &root, window, cx,
                        ),
                    }))
            }))
            .into_any_element()
    }

    fn refresh_button(&self, cx: &mut Context<Self>) -> IconButton {
        IconButton::new("svn-refresh-state", IconName::RotateCw)
            .icon_size(IconSize::Small)
            .tooltip(Tooltip::text(t!("Refresh")))
            .on_click(cx.listener(|this, _, window, cx| this.refresh(true, window, cx)))
    }

    fn workspace_placeholder(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.form.is_some() {
            return None;
        }
        if let Some(error) = &self.scan_error {
            return Some(
                panel_placeholder(t!("Error loading: {error}", error = error.as_str()), false)
                    .child(
                        Button::new("svn-retry-discovery", t!("Retry"))
                            .label_size(LabelSize::Small)
                            .style(ButtonStyle::Outlined)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh(true, window, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        if self.repo().is_some() {
            return None;
        }
        if self.scanning {
            return Some(
                panel_placeholder(t!("Scanning SVN working copies…"), true).into_any_element(),
            );
        }
        if self.history.visible {
            return Some(panel_placeholder(t!("No repository found"), false).into_any_element());
        }
        let no_project = self.project.read(cx).visible_worktrees(cx).next().is_none();
        Some(
            panel_placeholder(t!("No SVN working copies in this workspace."), false)
                .child(
                    h_flex()
                        .justify_center()
                        .flex_wrap()
                        .gap_1()
                        .when(no_project, |actions| {
                            actions.child(
                                Button::new("svn-open-project", t!("Open Project"))
                                    .label_size(LabelSize::Small)
                                    .style(ButtonStyle::Outlined)
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            workspace::Open::default().boxed_clone(),
                                            cx,
                                        );
                                    }),
                            )
                        })
                        .child(
                            Button::new("svn-empty-checkout", t!("SVN Checkout"))
                                .label_size(LabelSize::Small)
                                .style(ButtonStyle::Outlined)
                                .disabled(self.client.is_none() || self.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.start_form(form::Kind::Checkout, window, cx);
                                })),
                        )
                        .child(self.refresh_button(cx)),
                )
                .into_any_element(),
        )
    }

    fn selection_checkbox(
        &self,
        id: String,
        paths: Vec<String>,
        cx: &Context<Self>,
    ) -> Option<Checkbox> {
        if paths.is_empty() {
            return None;
        }
        let selected = paths
            .iter()
            .filter(|path| self.selected_paths.contains(*path))
            .count();
        let all = paths.len() == selected;
        Some(
            Checkbox::new(id, ToggleState::from_any_and_all(selected > 0, all))
                .fill()
                .elevation(ElevationIndex::Surface)
                .disabled(self.busy)
                .tooltip(Tooltip::text(if all {
                    t!("Clear Selection")
                } else {
                    t!("Select All")
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.focus.focus(window, cx);
                    this.select_paths(paths.clone(), !all, cx);
                })),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_directory_row(
        &self,
        section: Section,
        path: String,
        name: &str,
        depth: usize,
        change: Option<&Change>,
        paths: Vec<String>,
        collapsed: bool,
        root: &Path,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = format!("{}-{path}", section.test_id());
        let key = (section, path.clone());
        let active = self.active_directory.as_ref() == Some(&key);
        let colors = cx.theme().colors();
        let hover = colors.ghost_element_hover;
        let visible_path = path.clone();
        let versioned = change.is_some();
        let row = h_flex()
            .id(format!("svn-dir-{id}"))
            .w_full()
            .h(rems(1.75))
            .pl_2p5()
            .pr_1()
            .min_w_0()
            .gap_1p5()
            .cursor_pointer()
            .border_1()
            .border_r_2()
            .border_color(if active && self.focus.is_focused(window) {
                colors.panel_focused_border
            } else {
                gpui::transparent_black()
            })
            .when(active, |row| row.bg(colors.ghost_element_selected))
            .hover(move |row| row.bg(hover))
            .tooltip(Tooltip::text(path.clone()))
            .child(change_path_cell(
                true,
                depth,
                Some(!collapsed),
                Icon::new(if collapsed {
                    IconName::Folder
                } else {
                    IconName::FolderOpen
                })
                .size(IconSize::Small)
                .color(Color::Muted),
                Label::new(name.to_owned())
                    .single_line()
                    .truncate()
                    .when(
                        change.is_some_and(|entry| {
                            matches!(entry.item.as_str(), "missing" | "deleted")
                        }),
                        Label::strikethrough,
                    ),
            ))
            .when_some(change, |row, entry| row.child(status_icon(entry.label())))
            .children(self.selection_checkbox(format!("svn-dir-select-{id}"), paths, cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus.focus(window, cx);
                this.focus_directory(
                    section,
                    visible_path.clone(),
                    versioned,
                    Some(!collapsed),
                    cx,
                );
            }));
        let panel = cx.weak_entity();
        let absolute = root.join(&path);
        let change = change.cloned();
        right_click_menu(format!("svn-dir-menu-{id}"))
            .menu(move |window, cx| {
                if let Some(change) = change.clone() {
                    return Self::row_menu(
                        panel.clone(),
                        change,
                        true,
                        absolute.clone(),
                        window,
                        cx,
                    );
                }
                Self::path_menu(&path, &absolute, window, cx)
            })
            .trigger(move |_, _, _| row)
            .into_any_element()
    }

    fn focus_directory(
        &mut self,
        section: Section,
        path: String,
        versioned: bool,
        collapsed: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let key = (section, path.clone());
        self.active_path = versioned.then_some(path);
        self.active_directory = Some(key.clone());
        match collapsed {
            Some(true) => {
                self.collapsed_directories.insert(key);
            }
            Some(false) => {
                self.collapsed_directories.remove(&key);
            }
            None => {}
        }
        cx.notify();
    }

    fn path_menu(
        path: &str,
        absolute: &Path,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        ContextMenu::build(window, cx, |menu, _, _| {
            Self::copy_paths_menu(menu, path, absolute)
        })
    }

    fn copy_paths_menu(menu: ContextMenu, path: &str, absolute: &Path) -> ContextMenu {
        let path = path.to_owned();
        let absolute = absolute.display().to_string();
        menu.entry(t!("Copy Path"), None, move |_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(absolute.clone()));
        })
        .entry(t!("Copy Relative Path"), None, move |_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(path.clone()));
        })
    }

    fn render_change_row(
        &self,
        change: &Change,
        depth: usize,
        root: &Path,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = change.path.clone();
        let absolute = root.join(&path);
        let change_for_menu = change.clone();
        let is_dir = change.is_dir;
        let checked = self.selected_paths.contains(&path);
        let active = self.active_directory.is_none() && self.active_path.as_ref() == Some(&path);
        let colors = cx.theme().colors();
        let info = cx.theme().status().info;
        let focused = active && self.focus.is_focused(window);
        let panel = cx.weak_entity();
        let mut details = vec![path.clone()];
        for label in [
            change.conflicted().then(|| t!("Conflict")),
            change.switched.then(|| t!("Switched")),
            change.file_external.then(|| t!("External")),
            change.copied.then(|| t!("Copied")),
            (change.moved_from.is_some() || change.moved_to.is_some()).then(|| t!("Moved")),
            (change.remote_item.is_some() || change.remote_props.is_some())
                .then(|| t!("Remote Change")),
        ]
        .into_iter()
        .flatten()
        {
            details.push(label.to_string());
        }
        if let Some(changelist) = &change.changelist {
            details.push(changelist.clone());
        }
        let lock_details = change.locked.then(|| {
            let mut lines = vec![t!("SVN file lock").to_string()];
            if let Some(lock) = &change.lock {
                if let Some(owner) = &lock.owner {
                    lines.push(format!("{} · {owner}", t!("Lock owner")));
                }
                if let Some(comment) = lock.comment.as_ref().filter(|s| !s.trim().is_empty()) {
                    lines.push(format!("{} · {comment}", t!("Lock comment")));
                }
                lines.push(if lock.held_here {
                    t!("This working copy holds the SVN lock.").to_string()
                } else {
                    t!("This working copy does not hold the SVN lock.").to_string()
                });
            }
            lines.join("\n")
        });
        if let Some(lock_details) = &lock_details {
            details.push(lock_details.clone());
        }
        let tree = self.view_options.is_tree();
        let row = h_flex()
            .id(format!("svn-entry-{path}"))
            .h(rems(1.75))
            .w_full()
            .min_w_0()
            .pl_2p5()
            .pr_1()
            .gap_1p5()
            .border_1()
            .border_r_2()
            .cursor_pointer()
            .border_color(if focused {
                colors.panel_focused_border
            } else {
                gpui::transparent_black()
            })
            .bg(if active {
                info.alpha(0.08)
            } else {
                colors.ghost_element_background
            })
            .hover(|row| {
                row.bg(if active {
                    info.alpha(0.12)
                } else {
                    colors.ghost_element_hover
                })
            })
            .child(change_path_cell(
                tree,
                depth,
                None,
                status_icon(change.label()),
                path_label(
                    if tree {
                        path.rsplit('/').next().unwrap_or(&path)
                    } else {
                        &path
                    },
                    matches!(change.item.as_str(), "deleted" | "missing"),
                ),
            ))
            .when_some(lock_details, |row, lock_details| {
                row.child(
                    div()
                        .id(format!("svn-lock-{path}"))
                        .child(
                            Icon::new(IconName::Lock)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .tooltip(Tooltip::text(lock_details)),
                )
            })
            .when(
                change.remote_item.is_some() || change.remote_props.is_some(),
                |row| {
                    row.child(
                        Icon::new(IconName::ArrowDown)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                },
            )
            .when_some(change.diff_stat, |row, stat| {
                row.child(ui::DiffStat::new(
                    format!("svn-diff-stat-{path}"),
                    stat.added,
                    stat.deleted,
                ))
            })
            .when(change.item == "unversioned", |row| {
                let path = path.clone();
                row.child(
                    IconButton::new(format!("svn-add-{path}"), IconName::Plus)
                        .icon_size(IconSize::Small)
                        .disabled(self.busy)
                        .tooltip(Tooltip::text(t!("SVN Add")))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.active_path = Some(path.clone());
                            this.write_paths("add", vec![path.clone()], window, cx);
                        })),
                )
            })
            .when(change.committable(), |row| {
                let path = path.clone();
                row.child(
                    Checkbox::new(format!("svn-select-{path}"), checked.into())
                        .fill()
                        .elevation(ElevationIndex::Surface)
                        .disabled(self.busy)
                        .tooltip(Tooltip::text(t!("Include in SVN Commit")))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.focus.focus(window, cx);
                            this.select_change(path.clone(), !checked, cx)
                        })),
                )
            })
            .tooltip(Tooltip::text(details.join("\n")))
            .on_click(cx.listener({
                let change = change.clone();
                move |this, _, window, cx| {
                    if change.is_dir
                        || change.item == "external"
                        || change.can_view_diff() && !change.conflicted()
                    {
                        this.focus.focus(window, cx);
                    }
                    this.activate_change(change.clone(), window, cx);
                }
            }));
        if self.busy {
            row.into_any_element()
        } else {
            right_click_menu(format!("svn-row-menu-{path}"))
                .menu(move |window, cx| {
                    Self::row_menu(
                        panel.clone(),
                        change_for_menu.clone(),
                        is_dir,
                        absolute.clone(),
                        window,
                        cx,
                    )
                })
                .trigger(move |_, _, _| row)
                .into_any_element()
        }
    }

    fn activate_change(&mut self, change: Change, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.active_directory = None;
        self.active_path = Some(change.path.clone());
        if change.item == "external" && change.is_dir {
            if let Some(root) = self
                .selected_root
                .as_ref()
                .map(|root| root.join(&change.path))
            {
                self.switch_repository(root, window, cx);
            }
        } else if change.conflicted() && change.can_view_diff() {
            self.run(
                "read",
                json!({"operation":"conflict","path":change.path}),
                Output::Conflict,
                window,
                cx,
            );
        } else if change.can_view_diff() {
            self.show_worktree_diff(Some(change.moved_to.unwrap_or(change.path)), window, cx);
        } else if !change.is_dir && change.item != "external" {
            self.open_file(change.path, window, cx);
        }
        cx.notify();
    }

    fn change_keys(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus.is_focused(window)
            || self.busy
            || self.history.visible
            || self.form.is_some()
        {
            return;
        }
        let key = event.keystroke.key.as_ref();
        if matches!(key, "pageup" | "pagedown") {
            let height = self.scroll.bounds().size.height;
            let mut offset = self.scroll.offset();
            offset.y += if key == "pageup" { height } else { -height };
            self.scroll.set_offset(offset);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if !matches!(key, "up" | "down" | "left" | "right" | "enter" | "space") {
            return;
        }
        let groups = self.change_groups();
        let visible = groups
            .iter()
            .flat_map(|group| group.rows.iter().map(move |entry| (group.section, entry)))
            .collect::<Vec<_>>();
        if visible.is_empty() {
            return;
        }
        let current = visible
            .iter()
            .position(|(section, entry)| match entry.row(*section) {
                Row::Change(path) => {
                    self.active_directory.is_none() && self.active_path.as_deref() == Some(path)
                }
                Row::Directory(section, path) => self
                    .active_directory
                    .as_ref()
                    .is_some_and(|active| active.0 == section && active.1 == path),
            });
        let index = match (key, current) {
            ("up", Some(index)) => index.saturating_sub(1),
            ("up", None) => visible.len() - 1,
            ("down", Some(index)) => (index + 1).min(visible.len() - 1),
            (_, index) => index.unwrap_or(0),
        };
        let (section, entry) = visible[index];
        cx.stop_propagation();
        match entry {
            Entry::Change { change, .. } => {
                let change = (*change).clone();
                match key {
                    "enter" => self.activate_change(change, window, cx),
                    "space" if change.committable() => {
                        let checked = self.selected_paths.contains(&change.path);
                        self.select_change(change.path, !checked, cx);
                    }
                    "up" | "down" => {
                        self.active_directory = None;
                        self.active_path = Some(change.path);
                        cx.notify();
                    }
                    _ => {}
                }
            }
            Entry::Directory {
                path,
                change,
                paths,
                collapsed,
                ..
            } => {
                let (path, versioned) = (path.clone(), change.is_some());
                let next = match key {
                    "enter" => Some(!collapsed),
                    "left" => Some(true),
                    "right" => Some(false),
                    _ => None,
                };
                if key == "space" && !paths.is_empty() {
                    let paths = paths.clone();
                    let checked = paths.iter().all(|path| self.selected_paths.contains(path));
                    self.select_paths(paths, !checked, cx);
                } else {
                    self.focus_directory(section, path, versioned, next, cx);
                }
            }
        }
    }

    pub(super) fn commit_selection(&self) -> Vec<String> {
        self.repo()
            .into_iter()
            .flat_map(|repo| &repo.entries)
            .filter(|entry| entry.committable() && self.selected_paths.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect()
    }

    pub(super) fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.commit_generation.is_some() {
            return;
        }
        let Some(repo) = self.repo() else { return };
        let paths = self.commit_selection();
        let message = self.commit_editor.read(cx).text(cx);
        if paths.is_empty() || message.trim().is_empty() {
            return;
        }
        let mut detail = format!("{}\n{}", repo.copy.root.display(), paths.join("\n"));
        if repo.entries.iter().any(|entry| {
            paths.contains(&entry.path)
                && (entry.copied || matches!(entry.item.as_str(), "deleted" | "replaced"))
        }) {
            detail.push('\n');
            detail.push_str(&t!(
                "Directory copies, replacements, and deletions may affect the entire subtree."
            ));
        }
        self.confirm(
            t!("SVN Commit"),
            detail,
            "write",
            json!({"operation":"commit","paths":paths,"message":message}),
            Output::Commit,
            window,
            cx,
        );
    }

    fn update_actions(&self, ready: bool, cx: &Context<Self>) -> AnyElement {
        use form::Kind;
        let panel = cx.weak_entity();
        let options = PopoverMenu::new("svn-update-options")
            .trigger_with_tooltip(
                ButtonLike::new_rounded_right("svn-update-options-trigger")
                    .layer(ElevationIndex::ModalSurface)
                    .size(ButtonSize::Compact)
                    .disabled(!ready)
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall)),
                Tooltip::text(t!("SVN Update")),
            )
            .menu(move |window, cx| {
                Some(ContextMenu::build(window, cx, |menu, _, _| {
                    menu.item(Self::action_entry(
                        panel.clone(),
                        Kind::Update.label(),
                        MenuAction::Form(Kind::Update),
                        false,
                    ))
                    .item(Self::action_entry(
                        panel.clone(),
                        t!("Update All SVN Working Copies"),
                        MenuAction::UpdateAll,
                        false,
                    ))
                    .item(Self::action_entry(
                        panel.clone(),
                        t!("Check Remote Status"),
                        MenuAction::RemoteStatus,
                        false,
                    ))
                }))
            })
            .anchor(Anchor::TopRight);
        SplitButton::new(
            ButtonLike::new_rounded_left("svn-update")
                .layer(ElevationIndex::ModalSurface)
                .size(ButtonSize::Compact)
                .disabled(!ready)
                .child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(IconName::ArrowDown).size(IconSize::Small))
                        .child(Label::new(t!("Update")).size(LabelSize::Small).mr_0p5()),
                )
                .tooltip(Tooltip::text(t!("SVN Update")))
                .on_click(
                    cx.listener(|this, _, window, cx| this.simple_write("update", window, cx)),
                ),
            options.into_any_element(),
        )
        .into_any_element()
    }

    fn footer(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let repo = self.repo()?;
        let revision = format!("r{}", repo.copy.revision);
        let count = self.commit_selection().len();
        let ready = !self.busy && self.client.is_some();
        let enabled = ready
            && count > 0
            && self.commit_generation.is_none()
            && !self.commit_editor.read(cx).text(cx).trim().is_empty();
        let label = if count == 0 {
            t!("Commit Selected Changes")
        } else {
            format!("{} ({count})", t!("Commit")).into()
        };
        let border = cx.theme().colors().border;
        let editor = git_ui::git_panel::panel_editor_container(window, cx)
            .id("svn-commit-editor")
            .key_context("CommitEditor")
            .on_action(cx.listener(Self::generate_commit_message_action))
            .on_action(cx.listener(Self::open_commit_editor))
            .on_action(cx.listener(Self::toggle_commit_editor_size))
            .on_action(cx.listener(|this, _: &git::Commit, window, cx| {
                cx.stop_propagation();
                this.commit(window, cx);
            }))
            .h(gpui::Length::Auto)
            .when_else(
                self.commit_editor_expanded,
                |editor| editor.flex_1().min_h_0(),
                |editor| editor.flex_none(),
            )
            .border_t_1()
            .border_color(border)
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .when(self.commit_editor_expanded, |row| row.flex_1().min_h_0())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pt_2()
                            .px_2()
                            .when(self.commit_editor_expanded, |editor| editor.h_full())
                            .child(self.commit_editor.clone()),
                    )
                    .child(
                        v_flex()
                            .p_1()
                            .child(
                                IconButton::new("svn-open-commit-modal", IconName::MaximizeAlt)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(t!("Open Commit Modal")))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_commit_editor(
                                            &git::ExpandCommitEditor,
                                            window,
                                            cx,
                                        )
                                    })),
                            )
                            .child(
                                IconButton::new(
                                    "svn-fill-commit-editor",
                                    if self.commit_editor_expanded {
                                        IconName::Minimize
                                    } else {
                                        IconName::Maximize
                                    },
                                )
                                .icon_size(IconSize::Small)
                                .tooltip(Tooltip::text(if self.commit_editor_expanded {
                                    t!("Collapse Commit Editor")
                                } else {
                                    t!("Expand Commit Editor")
                                }))
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.toggle_commit_editor_size(
                                            &git::ToggleFillCommitEditor,
                                            window,
                                            cx,
                                        )
                                    },
                                )),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .p_1p5()
                    .justify_between()
                    .child(self.commit_message_controls(
                        cx.weak_entity(),
                        self.commit_editor.focus_handle(cx),
                        cx,
                    ))
                    .child(
                        ButtonLike::new("svn-commit")
                            .layer(ElevationIndex::ModalSurface)
                            .size(ButtonSize::Compact)
                            .disabled(!enabled)
                            .child(Label::new(label).size(LabelSize::Small))
                            .tooltip(Tooltip::text(if count == 0 {
                                t!("Select changes for the next SVN commit")
                            } else {
                                t!("SVN Commit")
                            }))
                            .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx))),
                    ),
            );
        Some(
            v_flex()
                .w_full()
                .when_else(
                    self.commit_editor_expanded,
                    |footer| footer.flex_1().min_h_0(),
                    |footer| footer.flex_none(),
                )
                .child(
                    h_flex()
                        .w_full()
                        .px_2()
                        .py_1p5()
                        .gap_1()
                        .justify_between()
                        .child(self.repository_switcher(cx))
                        .child(self.update_actions(ready, cx)),
                )
                .child(editor)
                .child(
                    h_flex()
                        .w_full()
                        .px_2()
                        .py_1p5()
                        .justify_between()
                        .border_t_1()
                        .border_color(border)
                        .child(
                            Label::new(revision)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .child(
                            IconButton::new("svn-footer-history", IconName::GitGraph)
                                .icon_size(IconSize::Small)
                                .disabled(!ready)
                                .tooltip(Tooltip::text(t!("Open SVN Log")))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_log(".", window, cx)
                                })),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Render for SvnPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .key_context("SvnPanel")
            .on_key_down(cx.listener(Self::change_keys))
            .when(!self.enabled, |panel| {
                panel.child(panel_placeholder(
                    t!("Install the Subversion extension to enable this panel."),
                    false,
                ))
            })
            .when(self.enabled, |panel| {
                panel
                    .child(self.history_tabs(cx))
                    .children(self.repository_section(window, cx))
                    .when(self.repo().is_some(), |panel| panel.child(self.toolbar(cx)))
                    .when(
                        self.history.visible || !self.commit_editor_expanded,
                        |panel| {
                            if let Some(placeholder) = self.workspace_placeholder(cx) {
                                return panel.child(placeholder);
                            }
                            panel.child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .min_w_0()
                                    .child(
                                        v_flex()
                                            .id("svn-panel-content")
                                            .size_full()
                                            .min_h_0()
                                            .min_w_0()
                                            .track_scroll(&self.scroll)
                                            .overflow_y_scroll()
                                            .when_some(self.form.as_ref(), |element, form| {
                                                element.child(self.render_form(form, cx))
                                            })
                                            .when(self.repo().is_some(), |element| {
                                                if self.history.visible {
                                                    element.child(self.render_history(window, cx))
                                                } else {
                                                    element.child(self.render_changes(window, cx))
                                                }
                                            }),
                                    )
                                    .vertical_scrollbar_for(&self.scroll, window, cx),
                            )
                        },
                    )
                    .when(!self.history.visible, |panel| {
                        panel.children(self.footer(window, cx))
                    })
            })
    }
}

#[derive(Clone)]
struct RepositoryRow {
    root: PathBuf,
    name: String,
    url: String,
    branch: String,
    count: usize,
    location: Option<String>,
}

impl From<&Repo> for RepositoryRow {
    fn from(repo: &Repo) -> Self {
        let root = repo.copy.root.clone();
        let name = root
            .file_name()
            .unwrap_or(root.as_os_str())
            .to_string_lossy()
            .into_owned();
        let branch = repo
            .copy
            .url
            .strip_prefix(&repo.copy.repository_url)
            .unwrap_or_default()
            .trim_matches('/');
        let branch = urlencoding::decode(branch)
            .map(|branch| branch.into_owned())
            .unwrap_or_else(|_| branch.to_owned());
        Self {
            root,
            name,
            url: repo.copy.url.clone(),
            branch,
            count: repo.change_count(),
            location: None,
        }
    }
}

impl RepositoryRow {
    fn tooltip(&self) -> String {
        format!("{}\n{}", self.root.display(), self.url)
    }

    fn short_branch(&self) -> String {
        if self.branch.is_empty() {
            return "^/".to_owned();
        }
        let mut parts = self.branch.split('/');
        while let Some(part) = parts.next() {
            match part {
                "trunk" => return part.to_owned(),
                "branches" => {
                    if let Some(name) = parts.next() {
                        return name.to_owned();
                    }
                }
                "tags" => {
                    if let Some(name) = parts.next() {
                        return format!("tags/{name}");
                    }
                }
                _ => {}
            }
        }
        self.branch.clone()
    }

    fn panel_labels(&self, active: bool) -> impl Iterator<Item = AnyElement> {
        [
            Icon::new(IconName::Subversion)
                .size(IconSize::Small)
                .color(if active { Color::Accent } else { Color::Muted })
                .into_any_element(),
            div()
                .min_w_0()
                .child(
                    Label::new(self.location.clone().unwrap_or_else(|| self.name.clone()))
                        .size(LabelSize::Small)
                        .truncate(),
                )
                .into_any_element(),
            div()
                .min_w_0()
                .flex_shrink(3.)
                .max_w(rems(7.5))
                .child(
                    Label::new(self.short_branch())
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .truncate_middle(),
                )
                .into_any_element(),
            div().flex_1().into_any_element(),
            if self.count == 0 {
                gpui::Empty.into_any_element()
            } else {
                Label::new(self.count.to_string())
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .flex_none()
                    .into_any_element()
            },
        ]
        .into_iter()
    }
}

struct RepositoryPicker {
    picker: Entity<picker::Picker<RepositoryPickerDelegate>>,
}

struct RepositoryPickerDelegate {
    panel: WeakEntity<SvnPanel>,
    parent: WeakEntity<RepositoryPicker>,
    active: PathBuf,
    rows: Vec<RepositoryRow>,
    matches: Vec<usize>,
    selected: usize,
}

impl RepositoryPicker {
    fn new(
        rows: Vec<RepositoryRow>,
        active: PathBuf,
        panel: WeakEntity<SvnPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let matches = (0..rows.len()).collect();
        let selected = rows.iter().position(|row| row.root == active).unwrap_or(0);
        let delegate = RepositoryPickerDelegate {
            panel,
            parent: cx.weak_entity(),
            active,
            rows,
            matches,
            selected,
        };
        let picker = cx.new(|cx| {
            picker::Picker::list(delegate, window, cx)
                .initial_width(rems(20.))
                .show_scrollbar(true)
        });
        Self { picker }
    }
}

impl EventEmitter<gpui::DismissEvent> for RepositoryPicker {}

impl Focusable for RepositoryPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for RepositoryPicker {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.picker.clone()
    }
}

impl picker::PickerDelegate for RepositoryPickerDelegate {
    type ListItem = ui::ListItem;

    fn name() -> &'static str {
        "SVN repository selector"
    }
    fn match_count(&self) -> usize {
        self.matches.len()
    }
    fn selected_index(&self) -> usize {
        self.selected
    }

    fn set_selected_index(
        &mut self,
        index: usize,
        _: &mut Window,
        cx: &mut Context<picker::Picker<Self>>,
    ) {
        self.selected = index.min(self.matches.len().saturating_sub(1));
        cx.notify();
    }

    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        t!("Select a repository...").as_str().into()
    }

    fn editor_position(&self) -> picker::PickerEditorPosition {
        picker::PickerEditorPosition::End
    }

    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<picker::Picker<Self>>,
    ) -> Task<()> {
        let query = query.to_lowercase();
        self.matches = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.tooltip().to_lowercase().contains(query.trim()))
            .map(|(index, _)| index)
            .collect();
        let active = self
            .matches
            .iter()
            .position(|index| self.rows[*index].root == self.active)
            .unwrap_or(0);
        self.set_selected_index(active, window, cx);
        Task::ready(())
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<picker::Picker<Self>>) {
        let Some(index) = self.matches.get(self.selected) else {
            return;
        };
        let root = self.rows[*index].root.clone();
        let _ = self
            .panel
            .update(cx, |panel, cx| panel.switch_repository(root, window, cx));
        self.dismissed(window, cx);
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<picker::Picker<Self>>) {
        let _ = self.parent.update(cx, |_, cx| cx.emit(gpui::DismissEvent));
    }

    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _: &mut Window,
        _: &mut Context<picker::Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let row = &self.rows[*self.matches.get(index)?];
        let active = row.root == self.active;
        Some(
            ui::ListItem::new(index)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .tooltip(Tooltip::text(row.tooltip()))
                .when(row.count > 0, |item| {
                    item.end_slot(
                        div().pr_2().child(
                            Label::new(row.count.to_string())
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    )
                })
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_1p5()
                        .child(
                            Icon::new(IconName::Subversion)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    h_flex()
                                        .min_w_0()
                                        .gap_1()
                                        .child(Label::new(row.name.clone()).truncate())
                                        .when(active, |line| {
                                            line.child(
                                                Icon::new(IconName::Check)
                                                    .size(IconSize::Small)
                                                    .color(Color::Accent),
                                            )
                                        })
                                        .child(
                                            div().min_w_0().flex_shrink(3.).child(
                                                Label::new(row.short_branch())
                                                    .size(LabelSize::Small)
                                                    .color(Color::Muted)
                                                    .truncate_middle(),
                                            ),
                                        ),
                                )
                                .when_some(row.location.clone(), |row, location| {
                                    row.child(
                                        Label::new(location)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted)
                                            .truncate_middle(),
                                    )
                                }),
                        ),
                ),
        )
    }
}
