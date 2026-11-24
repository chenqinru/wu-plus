use super::*;
use ui::ListItemSpacing;

#[derive(Deserialize)]
struct Target {
    path: String,
    url: String,
}

#[derive(Deserialize)]
struct Targets {
    targets: Vec<Target>,
    warning: Option<String>,
}

pub(super) struct BranchPicker {
    picker: Entity<picker::Picker<Delegate>>,
    request: Option<(Arc<rpc::Client>, u64, Task<()>)>,
}

struct Delegate {
    parent: WeakEntity<BranchPicker>,
    panel: WeakEntity<SvnPanel>,
    root: PathBuf,
    current_url: String,
    targets: Vec<Target>,
    matches: Vec<usize>,
    selected: usize,
    loading: bool,
    failed: bool,
}

impl BranchPicker {
    pub(super) fn new(
        panel: WeakEntity<SvnPanel>,
        copy: Copy,
        client: Arc<rpc::Client>,
        params: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = Delegate {
            parent: cx.weak_entity(),
            panel: panel.clone(),
            root: copy.root.clone(),
            current_url: copy.url.clone(),
            matches: Vec::new(),
            targets: Vec::new(),
            selected: 0,
            loading: true,
            failed: false,
        };
        let picker = cx.new(|cx| {
            picker::Picker::uniform_list(delegate, window, cx)
                .initial_width(rems(20.))
                .show_scrollbar(true)
                .popover()
        });
        let mut request = Some((panel, copy, client, params));
        // The authentication dialog must be able to restore focus to the open picker.
        cx.on_focus(&picker.focus_handle(cx), window, move |this, window, cx| {
            if let Some((panel, copy, client, params)) = request.take() {
                this.load(panel, copy, client, params, window, cx);
            }
        })
        .detach();
        Self {
            picker,
            request: None,
        }
    }

    fn load(
        &mut self,
        panel: WeakEntity<SvnPanel>,
        copy: Copy,
        client: Arc<rpc::Client>,
        params: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = client.next_id();
        let request_client = client.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let current = |panel: &SvnPanel, _: &App| {
                !panel.busy
                    && panel.repo().is_some_and(|repo| {
                        repo.copy.root == copy.root && repo.copy.url == copy.url
                    })
            };
            let result =
                authentication::request(&request_client, id, "read", params, &panel, current, cx)
                    .await
                    .and_then(|value| {
                        serde_json::from_value::<Targets>(value).map_err(decode_error)
                    });
            this.update_in(cx, |this, window, cx| {
                this.request = None;
                let targets = match result {
                    Ok(result) => {
                        if let Some(warning) = result.warning {
                            panel
                                .update(cx, |panel, cx| {
                                    if current(panel, cx) {
                                        panel.show_operation_result(
                                            None,
                                            String::new(),
                                            Some(warning),
                                            cx,
                                        );
                                    }
                                })
                                .log_err();
                        }
                        Some(result.targets)
                    }
                    Err(error) => {
                        if error.kind != "cancelled" {
                            panel
                                .update(cx, |panel, cx| {
                                    if current(panel, cx) {
                                        panel.set_error(
                                            t!("Switch Branch"),
                                            error,
                                            Some(copy.url.clone()),
                                            cx,
                                        );
                                    }
                                })
                                .log_err();
                        }
                        None
                    }
                };
                this.picker.update(cx, |picker, cx| {
                    picker.delegate.loading = false;
                    picker.delegate.failed = targets.is_none();
                    picker.delegate.targets = targets.unwrap_or_default();
                    picker.update_matches(picker.query(cx), window, cx);
                });
            })
            .log_err();
        });
        self.request = Some((client, id, task));
    }
}

impl Drop for BranchPicker {
    fn drop(&mut self) {
        if let Some((client, id, _)) = &self.request {
            client.cancel(*id);
        }
    }
}

impl EventEmitter<gpui::DismissEvent> for BranchPicker {}

impl Focusable for BranchPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for BranchPicker {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.picker.clone()
    }
}

impl picker::PickerDelegate for Delegate {
    type ListItem = ui::ListItem;

    fn name() -> &'static str {
        "SVN branch selector"
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
        t!("Switch Branch").as_str().into()
    }

    fn editor_position(&self) -> picker::PickerEditorPosition {
        picker::PickerEditorPosition::End
    }

    fn no_matches_text(&self, _: &mut Window, _: &mut App) -> Option<SharedString> {
        Some(if self.loading {
            t!("Loading")
        } else if self.failed {
            t!("Error loading: {error}", error = t!("Branches"))
        } else {
            t!("No Results")
        })
    }

    fn has_another_open_menu(&self, _: &Window, cx: &App) -> bool {
        self.panel
            .read_with(cx, |panel, cx| {
                panel
                    .workspace
                    .read_with(cx, |workspace, cx| {
                        workspace
                            .active_modal::<authentication::CredentialsModal>(cx)
                            .is_some()
                    })
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<picker::Picker<Self>>,
    ) -> Task<()> {
        let query = query.trim().to_lowercase();
        self.matches = self
            .targets
            .iter()
            .enumerate()
            .filter(|(_, target)| {
                target.path.to_lowercase().contains(&query)
                    || target.url.to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect();
        let current = self
            .matches
            .iter()
            .position(|index| self.targets[*index].url == self.current_url);
        self.set_selected_index(current.unwrap_or(0), window, cx);
        Task::ready(())
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<picker::Picker<Self>>) {
        let Some(index) = self.matches.get(self.selected) else {
            return;
        };
        let target = &self.targets[*index];
        let url = target.url.clone();
        let root = self.root.clone();
        let current_url = self.current_url.clone();
        let panel = self.panel.clone();
        self.dismissed(window, cx);
        if url != current_url {
            window.defer(cx, move |window, cx| {
                let _ = panel.update(cx, |panel, cx| {
                    if panel.busy
                        || !panel.repo().is_some_and(|repo| {
                            repo.copy.root == root && repo.copy.url == current_url
                        })
                    {
                        return;
                    }
                    panel.confirm(
                        form::Kind::Switch.label(),
                        format!("{}\n{url}", root.display()),
                        "write",
                        json!({"operation":"switch","url":url}),
                        Output::Change,
                        window,
                        cx,
                    );
                });
            });
        }
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
        let target = &self.targets[*self.matches.get(index)?];
        let current = target.url == self.current_url;
        let label: SharedString = if target.path.is_empty() {
            "^/".into()
        } else {
            target.path.clone().into()
        };
        Some(
            ui::ListItem::new(index)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .tooltip(Tooltip::text(target.url.clone()))
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2p5()
                        .child(
                            Icon::new(if current {
                                IconName::Check
                            } else {
                                IconName::GitBranch
                            })
                            .size(IconSize::Small)
                            .color(if current {
                                Color::Accent
                            } else {
                                Color::Muted
                            }),
                        )
                        .child(Label::new(label).truncate()),
                ),
        )
    }

    fn searchbar_trailer(
        &self,
        _: &mut Window,
        _: &mut Context<picker::Picker<Self>>,
    ) -> Option<AnyElement> {
        let panel = self.panel.clone();
        let parent = self.parent.clone();
        let root = self.root.clone();
        Some(
            Button::new("svn-switch-url", t!("SVN URL"))
                .size(ButtonSize::Compact)
                .label_size(LabelSize::Small)
                .tooltip(Tooltip::text(form::Kind::Switch.label()))
                .on_click(move |_, window, cx| {
                    let _ = parent.update(cx, |_, cx| cx.emit(gpui::DismissEvent));
                    window.defer(cx, {
                        let panel = panel.clone();
                        let root = root.clone();
                        move |window, cx| {
                            let _ = panel.update(cx, |panel, cx| {
                                if panel.selected_root.as_ref() == Some(&root) {
                                    panel.start_form(form::Kind::Switch, window, cx);
                                }
                            });
                        }
                    });
                })
                .into_any_element(),
        )
    }
}
