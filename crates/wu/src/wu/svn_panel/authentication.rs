use super::*;
use futures::channel::oneshot;
use gpui::{AsyncWindowContext, DismissEvent, Render};
use workspace::ModalView;

pub(super) async fn request(
    client: &Arc<rpc::Client>,
    id: u64,
    method: &str,
    params: Value,
    panel: &WeakEntity<SvnPanel>,
    current: impl Fn(&SvnPanel, &App) -> bool,
    cx: &mut AsyncWindowContext,
) -> Result<Value, rpc::Error> {
    let result = client.request(id, method, params.clone()).await;
    match result {
        Err(error) => retry(client, id, method, params, error, panel, current, cx).await,
        Ok(value) => Ok(value),
    }
}

pub(super) async fn retry(
    client: &Arc<rpc::Client>,
    id: u64,
    method: &str,
    mut params: Value,
    error: rpc::Error,
    panel: &WeakEntity<SvnPanel>,
    current: impl Fn(&SvnPanel, &App) -> bool,
    cx: &mut AsyncWindowContext,
) -> Result<Value, rpc::Error> {
    let writes = method == "write" && params["dry_run"].as_bool() != Some(true);
    let can_retry = if writes {
        error.kind == "authentication_before_write"
    } else {
        error.kind == "authentication"
    };
    if !can_retry {
        return Err(operation_error(error, writes));
    }

    let root = params["root"].as_str().map(PathBuf::from);
    let still_current = |panel: &SvnPanel, cx: &App| {
        let active_client = panel
            .client
            .as_ref()
            .or_else(|| panel.connecting.as_ref()?.peek()?.as_ref().ok());
        current(panel, cx) && active_client.is_some_and(|active| Arc::ptr_eq(active, client))
    };
    let workspace = panel
        .read_with(cx, |panel, app| {
            still_current(panel, app).then(|| panel.workspace.clone())
        })
        .ok()
        .flatten()
        .ok_or_else(cancelled)?;
    let repository_url = params["expected_url"].as_str().map(str::to_owned);
    let target = params["url"]
        .as_str()
        .or(params["source"].as_str())
        .or(repository_url.as_deref())
        .or_else(|| root.as_deref().and_then(Path::to_str))
        .unwrap_or_default()
        .to_owned();
    let username = params["auth"]["username"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let title = if method == "status" {
        t!("Check Remote Status")
    } else {
        operation_label(params["operation"].as_str())
    };
    let (sender, receiver) = oneshot::channel();
    let (_request_lifetime, request_ended) = oneshot::channel();
    let dry_run = method == "write" && !writes;
    let opened = cx
        .update(|window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    if dry_run {
                        let Some(form) = workspace.active_modal::<form::Form>(cx) else {
                            return false;
                        };
                        let modal = cx.new(|cx| {
                            CredentialsModal::new(title, target, username, true, sender, window, cx)
                                .dismiss_when_request_ends(request_ended, cx)
                        });
                        form.update(cx, |form, cx| form.show_authentication(modal, window, cx));
                    } else {
                        if workspace.has_active_modal(window, cx) {
                            return false;
                        }
                        workspace.toggle_modal(window, cx, |window, cx| {
                            CredentialsModal::new(title, target, username, true, sender, window, cx)
                                .dismiss_when_request_ends(request_ended, cx)
                        });
                    }
                    true
                })
                .unwrap_or(false)
        })
        .map_err(|_| cancelled())?;
    if !opened {
        return Err(if dry_run {
            cancelled()
        } else {
            operation_error(error, writes)
        });
    }
    let credentials = receiver.await.ok().flatten().ok_or_else(cancelled)?;
    if !panel.read_with(cx, still_current).unwrap_or(false) {
        return Err(cancelled());
    }

    params["auth"] = json!(&credentials);
    let result = client
        .request(id, method, params)
        .await
        .map_err(|error| operation_error(error, writes))?;
    if let Some(root) = root.or_else(|| result["destination"].as_str().map(PathBuf::from)) {
        panel
            .update(cx, |panel, cx| {
                if still_current(panel, cx)
                    && repository_url.as_ref().is_none_or(|url| {
                        panel
                            .repos
                            .iter()
                            .any(|repo| repo.copy.root == root && &repo.copy.url == url)
                    })
                {
                    panel.credentials.insert(
                        root,
                        Credentials {
                            remember: false,
                            ..credentials
                        },
                    );
                }
            })
            .log_err();
    }
    Ok(result)
}

fn operation_error(mut error: rpc::Error, writes: bool) -> rpc::Error {
    if writes && error.kind == "transport" {
        let summary = t!(
            "SVN operation outcome is unknown. Refresh and verify the repository before retrying."
        );
        error.message = format!("{summary}\n\n{}", error.message);
    } else if error.kind == "authentication_before_write" {
        error.kind = "authentication".into();
    }
    error
}

pub(super) fn cancelled() -> rpc::Error {
    rpc::Error {
        kind: "cancelled".into(),
        message: t!("Operation was canceled").to_string(),
    }
}

pub(super) fn operation_label(operation: Option<&str>) -> SharedString {
    match operation {
        Some("commit") => t!("SVN Commit"),
        Some("update") => t!("SVN Update"),
        Some("checkout") => form::Kind::Checkout.label(),
        Some("copy_branch") => form::Kind::CopyBranch.label(),
        Some("switch") => form::Kind::Switch.label(),
        Some("merge") => form::Kind::Merge.label(),
        Some("diff") => form::Kind::DiffRevision.label(),
        Some("history") => form::Kind::History.label(),
        Some("blame") => form::Kind::Blame.label(),
        Some("branches") => form::Kind::Branches.label(),
        Some("switch_targets") => t!("Switch Branch"),
        Some("lock") => form::Kind::Lock.label(),
        Some("unlock") => t!("SVN Unlock"),
        Some("revision_review") => t!("SVN History"),
        Some("worktree_diff") => t!("Working Copy Diff"),
        Some("conflict" | "resolve") => t!("Resolve SVN Conflict"),
        Some("add") => t!("SVN Add"),
        Some("delete") => t!("SVN Delete"),
        Some("move") => form::Kind::Move.label(),
        Some("revert") => t!("SVN Revert"),
        Some("cleanup") => t!("SVN Cleanup"),
        Some("changelist") => form::Kind::Changelist.label(),
        Some("properties") => t!("Show SVN Properties"),
        Some("set_property") => form::Kind::SetProperty.label(),
        _ => t!("SVN operation"),
    }
}

impl SvnPanel {
    pub(super) fn manage_credentials(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo() else {
            return;
        };
        let root = repo.copy.root.clone();
        let target = repo.copy.url.clone();
        let username = self
            .credentials
            .get(&root)
            .map(|credentials| credentials.username.clone())
            .unwrap_or_default();
        let workspace = self.workspace.clone();
        let (sender, receiver) = oneshot::channel();
        window.defer(cx, {
            let target = target.clone();
            move |window, cx| {
                workspace
                    .update(cx, |workspace, cx| {
                        workspace.toggle_modal(window, cx, |window, cx| {
                            CredentialsModal::new(
                                t!("SVN Credentials"),
                                target,
                                username,
                                false,
                                sender,
                                window,
                                cx,
                            )
                        });
                    })
                    .log_err();
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(credentials) = receiver.await.ok().flatten() else {
                return;
            };
            this.update_in(cx, |panel, window, cx| {
                if !panel
                    .repos
                    .iter()
                    .any(|repo| repo.copy.root == root && repo.copy.url == target)
                {
                    return;
                }
                panel.discard_history_for(&root);
                if credentials.username.is_empty() {
                    panel.credentials.remove(&root);
                    panel.show_status(t!("Using SVN command-line credentials."), false, cx);
                } else {
                    panel.credentials.insert(root.clone(), credentials);
                    panel.show_status(
                        t!("SVN credentials set for this working copy. Retry the operation."),
                        false,
                        cx,
                    );
                }
                if panel.selected_root.as_ref() == Some(&root) {
                    panel.error_kind = None;
                    panel.certificate_target = None;
                    panel.preload_history(false, window, cx);
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }
}

pub(super) struct CredentialsModal {
    operation: SharedString,
    target: String,
    username: Entity<Editor>,
    password: Entity<Editor>,
    required: bool,
    remember: bool,
    missing_username: bool,
    sender: Option<oneshot::Sender<Option<Credentials>>>,
    _request_lifetime: Option<Task<()>>,
}

impl CredentialsModal {
    fn new(
        operation: SharedString,
        target: String,
        username: String,
        required: bool,
        sender: oneshot::Sender<Option<Credentials>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let username = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text(&t!("Username"), window, cx);
            editor.set_text(username, window, cx);
            editor
        });
        let password = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text(&t!("Password"), window, cx);
            editor.set_masked(true, cx);
            editor
        });
        Self {
            operation,
            target,
            username,
            password,
            required,
            remember: false,
            missing_username: false,
            sender: Some(sender),
            _request_lifetime: None,
        }
    }

    fn dismiss_when_request_ends(
        mut self,
        request_ended: oneshot::Receiver<()>,
        cx: &mut Context<Self>,
    ) -> Self {
        self._request_lifetime = Some(cx.spawn(async move |modal, cx| {
            request_ended.await.ok();
            modal.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
        }));
        self
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.sender.take();
        cx.emit(DismissEvent);
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let username = self.username.read(cx).text(cx).trim().to_owned();
        let password = self.password.read(cx).text(cx);
        if username.is_empty() && (self.required || !password.is_empty()) {
            self.missing_username = true;
            self.username.focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        }
        if let Some(sender) = self.sender.take() {
            if sender
                .send(Some(Credentials {
                    username,
                    password: (!password.is_empty()).then_some(password),
                    remember: self.remember,
                }))
                .is_err()
            {
                log::debug!("The SVN operation ended before its credentials were submitted");
            }
        }
        cx.emit(DismissEvent);
    }
}

impl Focusable for CredentialsModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.username.focus_handle(cx)
    }
}

impl EventEmitter<DismissEvent> for CredentialsModal {}

impl ModalView for CredentialsModal {
    fn on_before_dismiss(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> workspace::DismissDecision {
        self.sender.take();
        workspace::DismissDecision::Dismiss(true)
    }
}

impl Render for CredentialsModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let field = |label: SharedString, editor: Entity<Editor>| {
            v_flex()
                .gap_1()
                .child(Label::new(label).size(LabelSize::Small))
                .child(
                    div()
                        .w_full()
                        .p_1()
                        .border_1()
                        .border_color(cx.theme().colors().border)
                        .rounded_sm()
                        .child(editor),
                )
        };
        v_flex()
            .key_context("menu")
            .elevation_3(cx)
            .w(rems(32.))
            .p_3()
            .gap_2()
            .on_action(cx.listener(|this, _: &menu::Cancel, _, cx| this.cancel(cx)))
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| this.submit(window, cx)))
            .child(Label::new(self.operation.clone()).weight(gpui::FontWeight::SEMIBOLD))
            .child(
                Label::new(self.target.clone())
                    .color(Color::Muted)
                    .truncate_middle(),
            )
            .child(field(t!("Username"), self.username.clone()))
            .when(self.missing_username, |element| {
                element.child(
                    Label::new(t!("Enter a value for {field}.", field = t!("Username")))
                        .color(Color::Error),
                )
            })
            .child(field(t!("Password"), self.password.clone()))
            .when(cfg!(target_os = "macos"), |element| {
                element.child(
                    h_flex()
                        .gap_1()
                        .child(
                            Checkbox::new("svn-remember", self.remember.into()).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.remember = !this.remember;
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(Label::new(t!("Save password to Keychain")).size(LabelSize::Small)),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("svn-cancel-auth", t!("Cancel"))
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                    )
                    .child(
                        Button::new(
                            "svn-submit-auth",
                            if self.required {
                                t!("Continue")
                            } else {
                                t!("Apply")
                            },
                        )
                        .style(ButtonStyle::Filled)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
