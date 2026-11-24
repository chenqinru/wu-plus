use super::*;
use gpui::{DismissEvent, Render};
use workspace::ModalView;

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
    panel: WeakEntity<SvnPanel>,
    root: Option<PathBuf>,
    working_copy_url: Option<String>,
    kind: Kind,
    paths: Vec<String>,
    path: String,
    missing_field: Option<usize>,
    error: Option<SharedString>,
    fields: Vec<(SharedString, Entity<Editor>)>,
    with_externals: bool,
    advanced: bool,
    choosing_directory: bool,
    authentication: Option<(Entity<authentication::CredentialsModal>, Subscription)>,
    preview: Option<Entity<Editor>>,
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

    fn fields(self) -> Vec<(SharedString, &'static str, bool)> {
        let url = || (t!("SVN URL"), "url", false);
        let rev = || (t!("Revision (optional)"), "revision", false);
        let message = || (t!("Commit Message"), "message", true);
        match self {
            Self::Checkout => vec![url(), rev()],
            Self::Update | Self::DiffRevision | Self::Blame => vec![rev()],
            Self::History => vec![(t!("Start Revision (optional)"), "before", false)],
            Self::Branches => vec![url()],
            Self::CopyBranch => vec![
                (t!("Source URL"), "source", false),
                (t!("Destination URL"), "destination", false),
                rev(),
                message(),
            ],
            Self::Switch => vec![url(), rev()],
            Self::Merge => vec![
                (t!("Source URL"), "source", false),
                (
                    t!("Revision Range (optional, e.g. 12:16)"),
                    "revisions",
                    false,
                ),
            ],
            Self::Move => vec![(t!("New Working Copy Relative Path"), "destination", false)],
            Self::Changelist => vec![(t!("Changelist Name"), "name", false)],
            Self::SetProperty => vec![
                (t!("Property Name"), "name", false),
                (t!("Property Value"), "value", true),
            ],
            Self::RemoveProperty => vec![(t!("Property Name"), "name", false)],
            Self::Lock => vec![(t!("Lock Comment (optional)"), "message", false)],
            Self::Credentials => Vec::new(),
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
        if kind == Kind::Credentials {
            self.manage_credentials(window, cx);
            return;
        }

        let root = self.selected_root.clone();
        let working_copy_url = self.repo().map(|repo| repo.copy.url.clone());
        let initial_url = self.repo().map(|repo| {
            if kind == Kind::Branches {
                repo.copy.repository_url.clone()
            } else {
                repo.copy.url.clone()
            }
        });
        let panel = cx.weak_entity();
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace.toggle_modal(window, cx, |window, cx| {
                        let fields = kind
                            .fields()
                            .into_iter()
                            .map(|(label, key, multiline)| {
                                let editor = cx.new(|cx| {
                                    let mut editor = if multiline {
                                        Editor::auto_height(3, 8, window, cx)
                                    } else {
                                        Editor::single_line(window, cx)
                                    };
                                    editor.set_placeholder_text(label.as_ref(), window, cx);
                                    if matches!(
                                        (kind, key),
                                        (Kind::Branches | Kind::Switch, "url")
                                            | (Kind::CopyBranch, "source")
                                    ) && let Some(url) = &initial_url
                                    {
                                        editor.set_text(url.clone(), window, cx);
                                    }
                                    editor
                                });
                                (label, editor)
                            })
                            .collect();
                        if let Some(panel) = panel.upgrade() {
                            cx.observe(&panel, |_, _, cx| cx.notify()).detach();
                        }
                        Form {
                            panel,
                            root,
                            working_copy_url,
                            kind,
                            paths,
                            path,
                            missing_field: None,
                            error: None,
                            fields,
                            with_externals: kind == Kind::Checkout,
                            advanced: false,
                            choosing_directory: false,
                            authentication: None,
                            preview: None,
                        }
                    });
                })
                .log_err();
        });
    }
}

impl Focusable for Form {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.authentication.as_ref().map_or_else(
            || {
                self.fields
                    .first()
                    .expect("parameter form has an editor")
                    .1
                    .focus_handle(cx)
            },
            |(modal, _)| modal.focus_handle(cx),
        )
    }
}

impl EventEmitter<DismissEvent> for Form {}
impl ModalView for Form {}

impl Form {
    pub(super) fn show_authentication(
        &mut self,
        modal: Entity<authentication::CredentialsModal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_focus = window.focused(cx);
        let subscription = cx.subscribe_in(
            &modal,
            window,
            move |this, _, _: &DismissEvent, window, cx| {
                this.authentication = None;
                if let Some(focus) = &previous_focus {
                    focus.focus(window, cx);
                }
                cx.notify();
            },
        );
        modal.focus_handle(cx).focus(window, cx);
        self.authentication = Some((modal, subscription));
        cx.notify();
    }

    pub(super) fn show_preview(
        &mut self,
        mut text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.trim().is_empty() {
            text = t!("No changes").to_string();
        }
        self.preview = Some(cx.new(|cx| {
            let mut editor = Editor::auto_height(3, 10, window, cx);
            editor.set_text(text, window, cx);
            editor.set_read_only(true);
            editor
        }));
        cx.notify();
    }

    fn submit(&mut self, alternate: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.choosing_directory
            || self
                .panel
                .read_with(cx, |panel, _| panel.busy)
                .unwrap_or(true)
        {
            return;
        }
        self.missing_field = None;
        self.error = None;
        let kind = self.kind;
        let mut params = json!({"path": self.path, "paths": self.paths});
        let fields = kind.fields();
        for ((_, editor), (_, key, _)) in self.fields.iter().zip(&fields) {
            let value = editor.read(cx).text(cx);
            params[*key] = if matches!(*key, "value" | "message") && !value.is_empty() {
                json!(value)
            } else if !value.trim().is_empty() {
                json!(value.trim())
            } else {
                Value::Null
            };
        }
        let required: &[&str] = match kind {
            Kind::Checkout => &["url"],
            Kind::CopyBranch => &["source", "destination", "message"],
            Kind::Switch => &["url"],
            Kind::Merge => &["source"],
            Kind::Move => &["destination"],
            Kind::Changelist | Kind::SetProperty | Kind::RemoveProperty => &["name"],
            _ => &[],
        };
        if let Some(index) = fields.iter().position(|(_, key, _)| {
            required.contains(key)
                && params[*key]
                    .as_str()
                    .is_none_or(|value| value.trim().is_empty())
        }) {
            self.fields[index].1.focus_handle(cx).focus(window, cx);
            self.missing_field = Some(index);
            cx.notify();
            return;
        }

        if matches!(kind, Kind::Checkout | Kind::Update) {
            params["with_externals"] = json!(self.with_externals);
        }
        if kind == Kind::Checkout {
            let Some(name) = params["url"].as_str().and_then(checkout_name) else {
                self.fields[0].1.focus_handle(cx).focus(window, cx);
                self.error = Some(t!(
                    "Enter a valid SVN URL without embedded credentials. Percent-encode spaces."
                ));
                cx.notify();
                return;
            };
            let destination = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some(t!("Select as Repository Destination")),
            });
            self.choosing_directory = true;
            cx.notify();
            cx.spawn_in(window, async move |this, cx| {
                let destination = destination.await;
                this.update_in(cx, |this, window, cx| {
                    this.choosing_directory = false;
                    match destination {
                        Ok(Ok(Some(mut paths))) => {
                            if let Some(parent) = paths.pop() {
                                params["destination"] = json!(parent.join(name));
                                this.dispatch(params, false, window, cx);
                            }
                        }
                        Ok(Err(error)) => this.error = Some(error.to_string().into()),
                        Err(error) => log::debug!("SVN directory picker closed: {error}"),
                        Ok(Ok(None)) => {}
                    }
                    cx.notify();
                })
                .log_err();
            })
            .detach();
        } else {
            self.dispatch(params, alternate, window, cx);
        }
    }

    fn dispatch(
        &mut self,
        mut params: Value,
        alternate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kind = self.kind;
        let panel = self.panel.clone();
        let root = self.root.clone();
        let working_copy_url = self.working_copy_url.clone();
        if !alternate {
            cx.emit(DismissEvent);
        }
        window.defer(cx, move |window, cx| {
            panel
                .update(cx, |panel, cx| {
                    if panel.busy
                        || (kind != Kind::Checkout
                            && (panel.selected_root != root
                                || panel.repo().map(|repo| &repo.copy.url)
                                    != working_copy_url.as_ref()))
                    {
                        return;
                    }
                    let (method, operation, output, confirm) = match kind {
                        Kind::Checkout => ("write", "checkout", Output::Checkout, false),
                        Kind::Update => ("write", "update", Output::Change, true),
                        Kind::DiffRevision => ("read", "diff", Output::Diff, false),
                        Kind::History => {
                            let path = params["path"].as_str().unwrap_or(".").to_owned();
                            panel.open_filtered_history(
                                path,
                                params["before"].as_str(),
                                window,
                                cx,
                            );
                            return;
                        }
                        Kind::Blame => ("read", "blame", Output::Blame, false),
                        Kind::Branches => ("read", "branches", Output::Text, false),
                        Kind::CopyBranch => ("write", "copy_branch", Output::Change, true),
                        Kind::Switch => ("write", "switch", Output::Change, true),
                        Kind::Merge => {
                            params["dry_run"] = json!(alternate);
                            let output = if alternate {
                                Output::MergePreview
                            } else {
                                Output::Text
                            };
                            ("write", "merge", output, !alternate)
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
                    params["operation"] = json!(operation);
                    if confirm {
                        let detail = root
                            .as_deref()
                            .into_iter()
                            .map(|root| root.display().to_string())
                            .chain(
                                params["paths"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(Value::as_str)
                                    .map(str::to_owned),
                            )
                            .chain(
                                params["destination"]
                                    .as_str()
                                    .or_else(|| params["url"].as_str())
                                    .or_else(|| params["source"].as_str())
                                    .map(str::to_owned),
                            )
                            .collect::<Vec<_>>()
                            .join("\n");
                        panel.confirm(kind.label(), detail, method, params, output, window, cx);
                    } else {
                        panel.run(method, params, output, window, cx);
                    }
                })
                .log_err();
        });
    }
}

fn checkout_name(address: &str) -> Option<String> {
    let url = url::Url::parse(address).ok()?;
    if !matches!(url.scheme(), "file" | "svn" | "svn+ssh" | "http" | "https")
        || url.password().is_some()
        || (!url.username().is_empty() && url.scheme() != "svn+ssh")
        || address
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return None;
    }
    let segment = url
        .path_segments()?
        .rev()
        .find(|segment| !segment.is_empty())
        .unwrap_or("repository");
    let name = urlencoding::decode(segment).ok()?;
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', ':', '\0']) {
        return None;
    }
    Some(name.into_owned())
}

impl Render for Form {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((modal, _)) = &self.authentication {
            return modal.clone().into_any_element();
        }
        let merge = self.kind == Kind::Merge;
        let checkout = self.kind == Kind::Checkout;
        let show_externals = (checkout && self.advanced) || self.kind == Kind::Update;
        let disabled = self.choosing_directory
            || self
                .panel
                .read_with(cx, |panel, _| panel.busy)
                .unwrap_or(true);
        let mut selected = self.paths.iter().take(6).cloned().collect::<Vec<_>>();
        if self.paths.len() > selected.len() {
            selected.push(format!("… (+{})", self.paths.len() - selected.len()));
        }
        let selected = selected.join("\n");
        v_flex()
            .key_context("menu")
            .elevation_3(cx)
            .w(rems(34.))
            .p_3()
            .gap_2()
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .on_action(
                cx.listener(|this, _: &menu::Confirm, window, cx| this.submit(false, window, cx)),
            )
            .child(Label::new(self.kind.label()).weight(gpui::FontWeight::SEMIBOLD))
            .when(!selected.is_empty(), |form| {
                form.child(Label::new(selected).color(Color::Muted))
            })
            .children(
                self.fields
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !checkout || *index == 0 || self.advanced)
                    .map(|(index, (label, field))| {
                        let invalid = self.missing_field == Some(index)
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
            .when_some(self.error.clone(), |form, error| {
                form.child(Label::new(error).color(Color::Error))
            })
            .when(checkout, |form| {
                form.child(
                    Button::new("svn-checkout-advanced", t!("Advanced Options"))
                        .start_icon(
                            Icon::new(if self.advanced {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(IconSize::Small),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.advanced = !this.advanced;
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            })
            .when(show_externals, |form| {
                form.child(
                    h_flex()
                        .gap_1()
                        .child(
                            Checkbox::new("svn-include-externals", self.with_externals.into())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.with_externals = !this.with_externals;
                                    cx.notify();
                                })),
                        )
                        .child(Label::new(t!("Include Externals")).size(LabelSize::Small)),
                )
            })
            .when_some(self.preview.clone(), |form, preview| {
                form.child(Label::new(t!("Preview Merge")).size(LabelSize::Small))
                    .child(
                        div()
                            .p_1()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .child(preview),
                    )
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("svn-cancel-form", t!("Cancel"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    )
                    .when(merge, |row| {
                        row.child(
                            Button::new("svn-preview-merge", t!("Preview Merge"))
                                .disabled(disabled)
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.submit(true, window, cx)
                                    }),
                                ),
                        )
                    })
                    .child(
                        Button::new(
                            "svn-submit",
                            if checkout {
                                t!("Continue")
                            } else {
                                t!("Apply")
                            },
                        )
                        .style(ButtonStyle::Filled)
                        .disabled(disabled)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.submit(false, window, cx)),
                        ),
                    ),
            )
            .into_any_element()
    }
}
