use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Checkout,
    Update,
    DiffRevision,
    History,
    Blame,
    Branches,
    CopyBranch,
    Switch,
    Merge,
    Move,
    Changelist,
    SetProperty,
    RemoveProperty,
    Lock,
    Credentials,
}

pub(super) struct Form {
    pub(super) kind: Kind,
    paths: Vec<String>,
    path: String,
    missing_field: Option<usize>,
    fields: Vec<(SharedString, Entity<Editor>)>,
    remember: bool,
    with_externals: bool,
}

impl Form {
    pub(super) fn first_field_id(&self) -> Option<gpui::EntityId> {
        self.fields.first().map(|(_, field)| field.entity_id())
    }
}

impl Kind {
    pub(super) fn label(self) -> SharedString {
        match self {
            Self::Checkout => t!("SVN Checkout"),
            Self::Update => t!("Update to Revision"),
            Self::DiffRevision => t!("Diff Revision"),
            Self::History => t!("SVN History"),
            Self::Blame => t!("SVN Blame"),
            Self::Branches => t!("Browse SVN URL"),
            Self::CopyBranch => t!("Create Branch or Tag"),
            Self::Switch => t!("SVN Switch"),
            Self::Merge => t!("SVN Merge"),
            Self::Move => t!("SVN Move"),
            Self::Changelist => t!("Set Changelist"),
            Self::SetProperty => t!("Set SVN Property"),
            Self::RemoveProperty => t!("Remove SVN Property"),
            Self::Lock => t!("SVN Lock"),
            Self::Credentials => t!("SVN Credentials"),
        }
    }

    fn fields(self) -> Vec<(SharedString, &'static str, bool, bool)> {
        let url = || (t!("SVN URL"), "url", false, false);
        let rev = || (t!("Revision (optional)"), "revision", false, false);
        let username = || (t!("Username"), "username", false, false);
        let password = || (t!("Password"), "password", false, true);
        let message = || (t!("Commit Message"), "message", true, false);
        match self {
            Self::Checkout => vec![
                url(),
                (t!("Absolute Destination Path"), "destination", false, false),
                rev(),
                username(),
                password(),
            ],
            Self::Update | Self::DiffRevision | Self::Blame => vec![rev()],
            Self::History => vec![(t!("Start Revision (optional)"), "before", false, false)],
            Self::Branches => vec![url()],
            Self::CopyBranch => vec![
                (t!("Source URL"), "source", false, false),
                (t!("Destination URL"), "destination", false, false),
                rev(),
                message(),
            ],
            Self::Switch => vec![url(), rev()],
            Self::Merge => vec![
                (t!("Source URL"), "source", false, false),
                (
                    t!("Revision Range (optional, e.g. 12:16)"),
                    "revisions",
                    false,
                    false,
                ),
            ],
            Self::Move => vec![(
                t!("New Working Copy Relative Path"),
                "destination",
                false,
                false,
            )],
            Self::Changelist => vec![(t!("Changelist Name"), "name", false, false)],
            Self::SetProperty => vec![
                (t!("Property Name"), "name", false, false),
                (t!("Property Value"), "value", true, false),
            ],
            Self::RemoveProperty => vec![(t!("Property Name"), "name", false, false)],
            Self::Lock => vec![(t!("Lock Comment (optional)"), "message", false, false)],
            Self::Credentials => vec![username(), password()],
        }
    }
}

impl SvnPanel {
    pub(super) fn start_form(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || (kind != Kind::Checkout && self.repo().is_none()) {
            return;
        }
        let path = self.read_path(cx);
        self.start_form_for(kind, path, false, window, cx);
    }

    pub(super) fn start_form_for(
        &mut self,
        kind: Kind,
        path: String,
        single: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy || (kind != Kind::Checkout && self.repo().is_none()) {
            return;
        }
        let mut paths = match kind {
            Kind::Checkout
            | Kind::Update
            | Kind::Branches
            | Kind::CopyBranch
            | Kind::Switch
            | Kind::Merge
            | Kind::Credentials => Vec::new(),
            Kind::Changelist | Kind::Lock if !single && !self.selected_paths.is_empty() => {
                self.selected_paths.iter().cloned().collect()
            }
            _ if path != "." => vec![path.clone()],
            _ => Vec::new(),
        };
        if matches!(kind, Kind::Lock | Kind::Changelist) {
            if let Some(repo) = self.repo() {
                paths.retain(|path| {
                    let local_file = repo.copy.root.join(path);
                    local_file.is_file()
                        && repo
                            .entries
                            .iter()
                            .find(|entry| &entry.path == path)
                            .is_none_or(|entry| {
                                !entry.file_external
                                    && !entry.conflicted()
                                    && !matches!(
                                        entry.item.as_str(),
                                        "added" | "deleted" | "ignored" | "unversioned"
                                    )
                            })
                });
            }
            if paths.is_empty() {
                self.show_status(t!("Select at least one path."), true, cx);
                cx.notify();
                return;
            }
        }
        let fields = kind
            .fields()
            .into_iter()
            .map(|(label, key, multiline, masked)| {
                let initial = match (kind, key) {
                    (Kind::Branches, "url") => {
                        self.repo().map(|repo| repo.copy.repository_url.as_str())
                    }
                    (Kind::CopyBranch, "source") | (Kind::Switch, "url") => {
                        self.repo().map(|repo| repo.copy.url.as_str())
                    }
                    (Kind::Credentials, "username") => self
                        .selected_root
                        .as_ref()
                        .and_then(|root| self.credentials.get(root))
                        .map(|auth| auth.username.as_str()),
                    _ => None,
                };
                let field = cx.new(|cx| {
                    let mut editor = if multiline {
                        Editor::auto_height(3, 8, window, cx)
                    } else {
                        Editor::single_line(window, cx)
                    };
                    editor.set_placeholder_text(label.as_ref(), window, cx);
                    if let Some(initial) = initial {
                        editor.set_text(initial.to_owned(), window, cx);
                    }
                    if masked {
                        editor.set_masked(true, cx);
                    }
                    editor
                });
                cx.observe(&field, |this, _, cx| {
                    if this
                        .form
                        .as_ref()
                        .is_some_and(|form| form.missing_field.is_some())
                    {
                        cx.notify();
                    }
                })
                .detach();
                (label, field)
            })
            .collect::<Vec<_>>();
        if let Some((_, first)) = fields.first() {
            first.focus_handle(cx).focus(window, cx);
        }
        self.form = Some(Form {
            kind,
            paths,
            path,
            missing_field: None,
            fields,
            remember: false,
            with_externals: kind == Kind::Checkout,
        });
        cx.notify();
    }

    fn submit_form(&mut self, alternate: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut form) = self.form.take() else {
            return;
        };
        form.missing_field = None;
        let kind = form.kind;
        let mut params = json!({"path": form.path, "paths":form.paths});
        for ((_, editor), (_, key, _, _)) in form.fields.iter().zip(kind.fields()) {
            let value = editor.read(cx).text(cx);
            params[key] = if matches!(key, "value" | "message") && !value.is_empty() {
                json!(value)
            } else if !value.trim().is_empty() {
                if key == "password" {
                    json!(value)
                } else {
                    json!(value.trim())
                }
            } else {
                Value::Null
            };
        }
        let required: &[&str] = match kind {
            Kind::Checkout => &["url", "destination"],
            Kind::CopyBranch => &["source", "destination", "message"],
            Kind::Switch => &["url"],
            Kind::Merge => &["source"],
            Kind::Move => &["destination"],
            Kind::Changelist | Kind::SetProperty | Kind::RemoveProperty => &["name"],
            Kind::Credentials if params["password"].is_string() => &["username"],
            _ => &[],
        };
        let fields = kind.fields();
        if let Some(index) = fields.iter().position(|(_, key, _, _)| {
            required.contains(key)
                && params[key]
                    .as_str()
                    .is_none_or(|value| value.trim().is_empty())
        }) {
            let (_, editor) = &form.fields[index];
            editor.focus_handle(cx).focus(window, cx);
            form.missing_field = Some(index);
            self.form = Some(form);
            cx.notify();
            return;
        }
        if kind == Kind::Credentials {
            if let Some(root) = self.selected_root.clone() {
                self.history_requests.invalidate(&root);
                if let Some(username) = params["username"].as_str() {
                    self.credentials.insert(
                        root,
                        Credentials {
                            username: username.to_owned(),
                            password: params["password"].as_str().map(str::to_owned),
                            remember: form.remember,
                        },
                    );
                    self.show_status(
                        t!("SVN credentials set for this working copy. Retry the operation."),
                        false,
                        cx,
                    );
                } else {
                    self.credentials.remove(&root);
                    self.show_status(t!("Using SVN command-line credentials."), false, cx);
                }
            }
            self.error_kind = None;
            self.certificate_target = None;
            cx.notify();
            return;
        }
        let (method, operation, output, confirm) = match kind {
            Kind::Checkout => {
                if params["username"].is_string() || params["password"].is_string() {
                    params["auth"] = json!({
                        "username":params["username"].as_str().unwrap_or_default(),
                        "password":params["password"],
                        "remember":form.remember,
                    });
                }
                params
                    .as_object_mut()
                    .expect("parameters")
                    .remove("password");
                ("write", "checkout", Output::Checkout, false)
            }
            Kind::Update => ("write", "update", Output::Change, true),
            Kind::DiffRevision => ("read", "diff", Output::Diff, false),
            Kind::History => {
                self.error_kind = None;
                self.certificate_target = None;
                self.history.open(&form.path);
                let close_form = if alternate { None } else { form.first_field_id() };
                self.form = Some(form);
                self.load_history_page(false, params["before"].as_str(), close_form, window, cx);
                return;
            }
            Kind::Blame => ("read", "blame", Output::Blame, false),
            Kind::Branches => ("read", "branches", Output::Text, false),
            Kind::CopyBranch => ("write", "copy_branch", Output::Change, true),
            Kind::Switch => ("write", "switch", Output::Change, true),
            Kind::Merge => {
                params["dry_run"] = json!(alternate);
                ("write", "merge", Output::Text, !alternate)
            }
            Kind::Move => ("write", "move", Output::Change, true),
            Kind::Changelist => ("write", "changelist", Output::Change, false),
            Kind::SetProperty => {
                if params["value"].is_null() {
                    params["value"] = json!("");
                }
                ("write", "set_property", Output::Change, false)
            }
            Kind::RemoveProperty => ("write", "set_property", Output::Change, true),
            Kind::Lock => ("write", "lock", Output::Change, false),
            Kind::Credentials => return,
        };
        if matches!(kind, Kind::Checkout | Kind::Update) {
            params["with_externals"] = json!(form.with_externals);
        }
        params["operation"] = json!(operation);
        params["close_form"] = json!(!alternate);
        if confirm {
            let mut targets = form.paths.clone();
            if let Some(destination) = params["destination"]
                .as_str()
                .or_else(|| params["url"].as_str())
                .or_else(|| params["source"].as_str())
            {
                targets.push(destination.to_owned());
            }
            let root = self
                .selected_root
                .as_ref()
                .map(|root| root.display().to_string())
                .unwrap_or_default();
            let detail = format!("{root}\n{}", targets.join("\n"));
            self.form = Some(form);
            self.confirm(kind.label(), detail, method, params, output, window, cx);
        } else {
            self.form = Some(form);
            self.run(method, params, output, window, cx);
        }
        cx.notify();
    }

    pub(super) fn render_form(&self, form: &Form, cx: &mut Context<Self>) -> AnyElement {
        let disabled = self.busy || (form.kind == Kind::History && self.history.is_loading());
        let cancel = if self.busy && self.repo().is_none() {
            self.cancel_operation(cx).into_any_element()
        } else {
            Button::new("svn-cancel-form", t!("Cancel"))
                .disabled(disabled)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.form = None;
                    cx.notify();
                }))
                .into_any_element()
        };
        let mut selected = form.paths.iter().take(6).cloned().collect::<Vec<_>>();
        if form.paths.len() > selected.len() {
            selected.push(format!("… (+{})", form.paths.len() - selected.len()));
        }
        let selected = selected.join("\n");
        let action = form.kind.label();
        let auth = matches!(form.kind, Kind::Checkout | Kind::Credentials);
        let merge = form.kind == Kind::Merge;
        let externals = matches!(form.kind, Kind::Checkout | Kind::Update);
        v_flex()
            .gap_2()
            .px_2()
            .pt_2()
            .pb_1()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(
                Label::new(action)
                    .size(LabelSize::Small)
                    .weight(gpui::FontWeight::SEMIBOLD),
            )
            .when(!selected.is_empty(), |form| {
                form.child(Label::new(selected).color(Color::Muted))
            })
            .children(
                form.fields
                    .iter()
                    .enumerate()
                    .map(|(index, (label, field))| {
                        let invalid = form.missing_field == Some(index)
                            && field.read(cx).text(cx).trim().is_empty();
                        v_flex()
                            .gap_1()
                            .child(Label::new(label.clone()).size(LabelSize::Small))
                            .child(
                                div()
                                    .w_full()
                                    .p_1()
                                    .border_1()
                                    .border_color(if invalid {
                                        cx.theme().status().error
                                    } else {
                                        cx.theme().colors().border
                                    })
                                    .rounded_sm()
                                    .child(field.clone()),
                            )
                            .when(invalid, |field| {
                                field.child(
                                    Label::new(t!(
                                        "Enter a value for {field}.",
                                        field = label.clone()
                                    ))
                                    .size(LabelSize::Small)
                                    .color(Color::Error),
                                )
                            })
                    }),
            )
            .when(cfg!(target_os = "macos") && auth, |element| {
                element.child(
                    h_flex()
                        .gap_1()
                        .child(
                            Checkbox::new("svn-remember", form.remember.into()).on_click(
                                cx.listener(|this, _, _, cx| {
                                    if let Some(form) = this.form.as_mut() {
                                        form.remember = !form.remember;
                                    }
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(Label::new(t!("Save password to Keychain")).size(LabelSize::Small)),
                )
            })
            .when(externals, |element| {
                element.child(
                    h_flex()
                        .gap_1()
                        .child(
                            Checkbox::new("svn-include-externals", form.with_externals.into())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(form) = this.form.as_mut() {
                                        form.with_externals = !form.with_externals;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(Label::new(t!("Include Externals")).size(LabelSize::Small)),
                )
            })
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .child(
                        Button::new("svn-submit", t!("Apply"))
                            .style(ButtonStyle::Filled)
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_form(false, window, cx)
                            })),
                    )
                    .when(merge, |element| {
                        element.child(
                            Button::new("svn-preview-merge", t!("Preview Merge"))
                                .disabled(disabled)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_form(true, window, cx)
                                })),
                        )
                    })
                    .when(self.certificate_recovery().is_some(), |actions| {
                        actions.child(
                            Button::new(
                                "svn-form-verify-certificate",
                                t!("Verify SVN Server Certificate"),
                            )
                            .disabled(disabled)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.certificate(window, cx)),
                            ),
                        )
                    })
                    .child(cancel),
            )
            .into_any_element()
    }
}
