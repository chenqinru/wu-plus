use super::*;
use editor::{Addon, GitBlame, HiddenUnstagedDiffHunkRenderer};
use git::blame::BlameEntry;
use language::{Buffer, BufferEvent};
use project::ProjectItem as _;
use std::{any::Any, path::Path, rc::Rc};

struct Attachment(Entity<FileHistory>);

impl Addon for Attachment {
    fn to_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone)]
struct Source {
    root: PathBuf,
    path: String,
    client: Arc<rpc::Client>,
}

struct FileHistory {
    editor: WeakEntity<Editor>,
    buffer: Entity<Buffer>,
    panel: WeakEntity<SvnPanel>,
    provider: Provider,
    path: PathBuf,
    source: Option<Source>,
    base: Option<Arc<str>>,
    installed_diff: Option<gpui::EntityId>,
    wants_blame: bool,
    locate: Option<Task<()>>,
    read_base: Option<Task<()>>,
    recalculate: Option<Task<()>>,
    read_blame: Option<Task<()>>,
    _buffer_subscription: Subscription,
}

impl SvnPanel {
    pub(super) fn recheck_editor_locations(
        &self,
        changed_roots: &BTreeSet<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editors = self
            .editor_attachments(cx)
            .into_iter()
            .filter(|state| {
                let state = state.read(cx);
                changed_roots.iter().any(|root| {
                    state.path.starts_with(root)
                        || state.source.as_ref().is_some_and(|source| &source.root == root)
                })
            })
            .collect::<Vec<_>>();
        if editors.is_empty() {
            return;
        }
        window.defer(cx, move |window, cx| {
            for state in editors {
                state.update(cx, |state, cx| {
                    state.detach(window, cx);
                    state.locate(window, cx);
                });
            }
        });
    }

    pub(super) fn bind_active_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let Some(editor) = workspace
            .read(cx)
            .active_item(cx)
            .and_then(|item| item.downcast::<Editor>())
        else {
            return;
        };
        let Some(provider) = self.provider.clone() else {
            return;
        };
        let Some(buffer) = editor.read(cx).buffer().read(cx).as_singleton() else {
            return;
        };
        let Some(path) = buffer
            .read(cx)
            .project_path(cx)
            .and_then(|path| self.project.read(cx).absolute_path(&path, cx))
        else {
            return;
        };
        let existing = editor
            .read(cx)
            .addon::<Attachment>()
            .map(|addon| addon.0.clone());
        if let Some(existing) = existing {
            if existing.read(cx).provider == provider && existing.read(cx).path == path {
                let state = existing.read(cx);
                if state.source.is_none() && state.locate.is_none() {
                    existing.update(cx, |state, cx| state.locate(window, cx));
                }
                return;
            }
            existing.update(cx, |state, cx| state.detach(window, cx));
        }
        let editor_handle = editor.downgrade();
        let panel = cx.weak_entity();
        let state = cx.new(|cx| {
            let subscription = cx.subscribe_in(
                &buffer,
                window,
                |this: &mut FileHistory, buffer, event, window, cx| match event {
                    BufferEvent::Edited { .. } => this.recalculate(cx),
                    BufferEvent::Reloaded => {
                        this.load_base(window, cx);
                        if this.wants_blame {
                            this.load_blame(false, window, cx);
                        }
                    }
                    BufferEvent::Saved | BufferEvent::DirtyChanged
                        if this.wants_blame && !buffer.read(cx).is_dirty() =>
                    {
                        this.load_blame(false, window, cx);
                    }
                    BufferEvent::FileHandleChanged => {
                        let panel = this.panel.clone();
                        window.defer(cx, move |window, cx| {
                            if let Some(panel) = panel.upgrade() {
                                panel.update(cx, |panel, cx| panel.bind_active_editor(window, cx));
                            }
                        });
                    }
                    _ => {}
                },
            );
            FileHistory {
                editor: editor_handle,
                buffer,
                panel,
                provider,
                path,
                source: None,
                base: None,
                installed_diff: None,
                wants_blame: false,
                locate: None,
                read_base: None,
                recalculate: None,
                read_blame: None,
                _buffer_subscription: subscription,
            }
        });
        editor.update(cx, |editor, _| {
            editor.register_addon(Attachment(state.clone()))
        });
        state.update(cx, |state, cx| state.locate(window, cx));
    }

    pub(super) fn refresh_editor_baselines(
        &self,
        root: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editors = self
            .editor_attachments(cx)
            .into_iter()
            .filter(|state| {
                state
                    .read(cx)
                    .source
                    .as_ref()
                    .is_some_and(|source| root.is_none_or(|root| source.root == root))
            })
            .collect::<Vec<_>>();
        window.defer(cx, move |window, cx| {
            for state in editors {
                state.update(cx, |state, cx| {
                    state.load_base(window, cx);
                    if state.wants_blame {
                        state.load_blame(false, window, cx);
                    }
                });
            }
        });
    }

    pub(super) fn detach_editor_history(&self, window: &mut Window, cx: &mut Context<Self>) {
        for state in self.editor_attachments(cx) {
            state.update(cx, |state, cx| state.detach(window, cx));
        }
    }

    fn editor_attachments(&self, cx: &App) -> Vec<Entity<FileHistory>> {
        self.workspace.upgrade().map_or_else(Vec::new, |workspace| {
            workspace
                .read(cx)
                .items(cx)
                .filter_map(|item| item.downcast::<Editor>())
                .filter_map(|editor| {
                    editor
                        .read(cx)
                        .addon::<Attachment>()
                        .map(|addon| addon.0.clone())
                })
                .collect()
        })
    }
}

fn svn_path(absolute: &Path) -> Option<(PathBuf, String)> {
    for directory in absolute.parent()?.ancestors() {
        if directory.join(".git").exists() {
            return None;
        }
        if directory.join(".svn").is_dir() {
            let path = super::svn_path_string(absolute.strip_prefix(directory).ok()?)?;
            return Some((directory.to_owned(), path));
        }
    }
    None
}

impl FileHistory {
    fn locate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let absolute = self.path.clone();
        let probe = cx.background_spawn(async move { svn_path(&absolute) });
        let panel = self.panel.clone();
        let provider = self.provider.clone();
        self.locate = Some(cx.spawn_in(window, async move |state, cx| {
            let located = probe.await;
            let Some((root, path)) = located else {
                return;
            };
            let Ok(Some(connection)) = panel.update(cx, |panel, cx| {
                if panel.provider.as_ref() == Some(&provider) {
                    panel.connection(cx)
                } else {
                    None
                }
            }) else {
                return;
            };
            let outcome = connection.await;
            state
                .update_in(cx, |state, window, cx| {
                    state.locate = None;
                    match outcome {
                        Ok(client) => {
                            state.source = Some(Source { root, path, client });
                            state.install_blame_loader(window, cx);
                            state.load_base(window, cx);
                        }
                        Err(error) => {
                            log::debug!("SVN editor connection failed: {}", error.message);
                            panel.update(cx, |panel, _| panel.connecting = None).ok();
                        }
                    }
                })
                .ok();
        }));
    }

    fn install_blame_loader(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = cx.weak_entity();
        let loader = Rc::new(move |user_triggered, window: &mut Window, cx: &mut App| {
            let state = state.clone();
            window.defer(cx, move |window, cx| {
                state
                    .update(cx, |state, cx| state.load_blame(user_triggered, window, cx))
                    .ok();
            });
        });
        self.editor
            .update(cx, |editor, cx| {
                editor.set_external_blame_loader(Some(loader), window, cx);
            })
            .ok();
    }

    fn load_base(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let buffer = self.buffer.clone();
        let request = cx.background_spawn(async move {
            source
                .client
                .request(
                    source.client.next_id(),
                    "read",
                    json!({
                        "operation":"editor_base", "root":source.root, "path":source.path,
                    }),
                )
                .await
        });
        self.read_base = Some(cx.spawn_in(window, async move |state, cx| {
            let reply = request.await;
            let Ok(reply) = reply else {
                return;
            };
            let base = reply["base"].as_str().map(normalize_lines);
            let changed = state
                .read_with(cx, |state, _| state.base.as_deref() != base.as_deref())
                .unwrap_or(false);
            if !changed {
                return;
            }
            if let Some(base) = base {
                let snapshot = cx.new(|cx| Buffer::local(base.as_ref(), cx));
                let Ok(diff) =
                    git_ui_core::file_diff_view::build_buffer_diff(&snapshot, &buffer, cx).await
                else {
                    return;
                };
                state
                    .update(cx, |state, cx| {
                        state.base = Some(base);
                        state.installed_diff = Some(diff.entity_id());
                        state
                            .editor
                            .update(cx, |editor, cx| {
                                editor.set_diff_hunk_renderer(
                                    Some(Arc::new(HiddenUnstagedDiffHunkRenderer)),
                                    cx,
                                );
                                editor
                                    .buffer()
                                    .update(cx, |buffer, cx| buffer.add_diff(diff, cx));
                            })
                            .ok();
                        state.recalculate(cx);
                    })
                    .ok();
            } else {
                state.update(cx, |state, cx| state.clear_diff(cx)).ok();
            }
        }));
    }

    fn recalculate(&mut self, cx: &mut Context<Self>) {
        let (Some(base), Some(installed_diff)) = (self.base.clone(), self.installed_diff) else {
            return;
        };
        let editor = self.editor.clone();
        let buffer = self.buffer.clone();
        self.recalculate.take();
        self.recalculate = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let task = editor
                .read_with(cx, |editor, cx| {
                    editor
                        .buffer()
                        .read(cx)
                        .diff_for(buffer.read(cx).remote_id())
                        .filter(|diff| diff.entity_id() == installed_diff)
                })
                .ok()
                .flatten();
            if let Some(diff) = task {
                let snapshot = buffer.read_with(cx, |buffer, _| buffer.text_snapshot());
                diff.update(cx, |diff, cx| diff.set_base_text(Some(base), snapshot, cx))
                    .await;
            }
        }));
    }

    fn clear_diff(&mut self, cx: &mut Context<Self>) {
        let Some(installed_diff) = self.installed_diff.take() else {
            return;
        };
        self.recalculate.take();
        let buffer = self.buffer.read(cx);
        let text = buffer.text_snapshot();
        let base = text
            .text_for_range(0..text.len())
            .collect::<String>()
            .into();
        let id = buffer.remote_id();
        self.base = None;
        let task = self
            .editor
            .update(cx, |editor, cx| {
                editor.set_diff_hunk_renderer(None, cx);
                editor
                    .buffer()
                    .read(cx)
                    .diff_for(id)
                    .filter(|diff| diff.entity_id() == installed_diff)
                    .map(|diff| {
                        diff.update(cx, |diff, cx| diff.set_base_text(Some(base), text, cx))
                    })
            })
            .ok()
            .flatten();
        if let Some(task) = task {
            task.detach();
        }
    }

    fn detach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.locate = None;
        self.read_base = None;
        self.read_blame = None;
        let was_attached = self.source.take().is_some();
        self.clear_diff(cx);
        if was_attached {
            self.editor
                .update(cx, |editor, cx| {
                    editor.set_external_blame_loader(None, window, cx);
                })
                .ok();
        }
    }

    fn load_blame(&mut self, user_triggered: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        self.wants_blame = true;
        let auth = self
            .panel
            .read_with(cx, |panel, _| {
                panel
                    .credentials
                    .get(&source.root)
                    .map(|credentials| json!(credentials))
            })
            .ok()
            .flatten();
        let mut params = json!({
            "operation":"blame", "root":source.root, "path":source.path, "editor":true,
        });
        if let Some(auth) = auth {
            params["auth"] = auth;
        }
        self.read_blame = Some(cx.spawn_in(window, async move |state, cx| {
            if !user_triggered {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            let reply = cx
                .background_spawn(async move {
                    source
                        .client
                        .request(source.client.next_id(), "read", params)
                        .await
                })
                .await;
            state
                .update_in(cx, |state, window, cx| {
                    let data = match reply {
                        Ok(reply) => {
                            serde_json::from_value::<BlameData>(reply["blame"].clone()).ok()
                        }
                        Err(error) => {
                            if user_triggered {
                                let message = error.message;
                                if let Some(project) = state
                                    .editor
                                    .read_with(cx, |editor, _| editor.project().cloned())
                                    .ok()
                                    .flatten()
                                {
                                    project.update(cx, |_, cx| {
                                        cx.emit(project::Event::Toast {
                                            notification_id: "svn-blame".into(),
                                            message,
                                            link: None,
                                        })
                                    });
                                }
                            }
                            return;
                        }
                    };
                    let Some(data) = data else {
                        return;
                    };
                    let snapshot = state.buffer.read(cx).text_snapshot();
                    if normalize_lines(&data.content).as_ref()
                        != snapshot
                            .text_for_range(0..snapshot.len())
                            .collect::<String>()
                    {
                        return;
                    }
                    let last_row = snapshot.max_point().row;
                    let entries = data
                        .lines
                        .into_iter()
                        .filter_map(|line| line.into_entry(&state.path, last_row))
                        .collect::<Vec<_>>();
                    let buffer = state.buffer.clone();
                    state
                        .editor
                        .update(cx, |editor, cx| {
                            let Some(project) = editor.project().cloned() else {
                                return;
                            };
                            let multi_buffer = editor.buffer().clone();
                            let blame = cx.new(|cx| {
                                GitBlame::new_external(multi_buffer, project, buffer, entries, cx)
                            });
                            editor.set_external_blame(blame, window, cx);
                        })
                        .ok();
                })
                .ok();
        }));
    }
}

fn normalize_lines(text: &str) -> Arc<str> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n").into()
    } else {
        text.into()
    }
}

#[derive(Deserialize)]
struct BlameData {
    content: String,
    lines: Vec<BlameLine>,
}

#[derive(Deserialize)]
struct BlameLine {
    line: u32,
    end_line: u32,
    revision: String,
    author: String,
    date: String,
}

impl BlameLine {
    fn into_entry(self, path: &Path, last_row: u32) -> Option<BlameEntry> {
        let start = self.line.checked_sub(1)?;
        let end = self.end_line.min(last_row.saturating_add(1));
        if start >= end {
            return None;
        }
        let revision: u64 = self.revision.parse().ok()?;
        // Native blame uses a per-revision key to group rows and derive colors.
        // This key never goes to Git or appears in the UI.
        let mut group = [0; 20];
        group[..8].copy_from_slice(&revision.to_le_bytes());
        let sha = git::Oid::from_bytes(&group).ok()?;
        let date =
            time::OffsetDateTime::parse(&self.date, &time::format_description::well_known::Rfc3339)
                .ok();
        let summary = date.map_or_else(
            || format!("r{revision}"),
            |date| {
                let timestamp = time_format::format_local_timestamp(
                    date,
                    time::OffsetDateTime::now_utc(),
                    time_format::TimestampFormat::Relative,
                );
                format!("r{revision} · {timestamp}")
            },
        );
        Some(BlameEntry {
            sha,
            range: start..end,
            original_line_number: self.line,
            author: Some(self.author),
            author_mail: None,
            author_time: date.map(|date| date.unix_timestamp()),
            author_tz: None,
            committer_name: None,
            committer_email: None,
            committer_time: None,
            committer_tz: None,
            summary: Some(summary),
            previous: None,
            filename: path.file_name()?.to_string_lossy().into_owned(),
            boundary: false,
        })
    }
}
