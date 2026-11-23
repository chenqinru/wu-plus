use super::*;
use editor::{EditorEvent, SplittableEditor};
use git_ui_core::file_diff_view::{FileDiffView, build_buffer_diff};
use gpui::{AnyElement, AnyEntity, EntityId, SharedString};
use language::{Buffer, BufferEvent, Capability};
use std::any::{Any, TypeId};
use workspace::{
    Item, ItemNavHistory,
    item::{ItemEvent, SaveOptions},
    searchable::SearchableItemHandle,
};

#[derive(Deserialize)]
struct Report {
    views: BTreeMap<String, String>,
    text: String,
    #[serde(default)]
    text_conflict: bool,
    #[serde(default)]
    property_conflict: bool,
    tree: Option<TreeConflict>,
    tree_incoming_revision: Option<u64>,
}

#[derive(Deserialize)]
struct TreeConflict {
    reason: String,
    action: String,
    operation: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Text,
    Properties,
    Tree,
    Details,
}

impl Section {
    fn label(self) -> SharedString {
        match self {
            Self::Text => t!("Text Conflict"),
            Self::Properties => t!("Property Conflict"),
            Self::Tree => t!("Tree Conflict"),
            Self::Details => t!("Details"),
        }
    }
}

impl Report {
    fn sections(&self) -> Vec<Section> {
        let mut sections = Vec::new();
        if self.text_conflict || self.views.contains_key("mine") {
            sections.push(Section::Text);
        }
        if self.property_conflict || self.views.contains_key("property_reject") {
            sections.push(Section::Properties);
        }
        if self.tree.is_some() {
            sections.push(Section::Tree);
        }
        sections
    }

    fn comparison(&self, section: Section) -> Option<(&str, &str)> {
        let (left, right) = match section {
            Section::Text => ("mine", "working"),
            Section::Properties => ("property_mine", "property_incoming"),
            Section::Tree => ("working", "tree_incoming"),
            Section::Details => return None,
        };
        Some((self.views.get(left)?, self.views.get(right)?))
    }
}

pub(super) fn open(
    workspace: WeakEntity<Workspace>,
    panel: WeakEntity<SvnPanel>,
    scope: preview::Scope,
    root: PathBuf,
    path: String,
    value: Value,
    window: &mut Window,
    cx: &mut App,
) {
    let Ok(report) = serde_json::from_value::<Report>(value) else {
        return;
    };
    window
        .spawn(cx, async move |cx| {
            let project = workspace.read_with(cx, |workspace, _| workspace.project().clone())?;
            let mut sections = report.sections();
            let is_mixed = sections.len() > 1;
            sections.push(Section::Details);
            let mut working = None;
            let mut comparisons = Vec::new();
            for section in sections.iter().copied() {
                let Some((left, right)) = report.comparison(section) else {
                    continue;
                };
                let languages = project.read_with(cx, |project, _| project.languages().clone());
                let language = if section == Section::Properties {
                    None
                } else {
                    languages
                        .load_language_for_file_path(std::path::Path::new(&path))
                        .await
                        .ok()
                };
                let snapshot = |text: &str, cx: &mut Context<Buffer>| {
                    let mut buffer = Buffer::local(text, cx);
                    buffer.set_capability(Capability::ReadOnly, cx);
                    buffer.set_language_registry(languages.clone());
                    buffer.set_language(language.clone(), cx);
                    buffer
                };
                let left = cx.new(|cx| snapshot(left, cx));
                let right = if section == Section::Text {
                    let target = root.join(&path);
                    if !smol::fs::symlink_metadata(&target)
                        .await
                        .is_ok_and(|metadata| metadata.is_file())
                    {
                        continue;
                    }
                    let buffer = project
                        .update(cx, |project, cx| project.open_local_buffer(&target, cx))
                        .await?;
                    working = Some(buffer.clone());
                    buffer
                } else {
                    cx.new(|cx| snapshot(right, cx))
                };
                let diff = build_buffer_diff(&left, &right, cx).await?;
                comparisons.push((section, left, right, diff));
            }
            if !cx.update(|_, cx| scope.is_current(cx))? {
                return anyhow::Ok(());
            }
            workspace.update_in(cx, |workspace, window, cx| {
                let workbench = cx.entity();
                let title = format!("{} · {path}", t!("Conflict"));
                let details = report.views.get("property_reject").map_or_else(
                    || report.text.clone(),
                    |reject| format!("{reject}\n\n{}", report.text),
                );
                let details = preview::view(t!("Details").as_ref(), &details, window, cx);
                let item = cx.new(|cx| {
                    let mut subscriptions = Vec::new();
                    if let Some(panel) = panel.upgrade() {
                        subscriptions.push(cx.observe(&panel, |_, _, cx| cx.notify()));
                    }
                    if let Some(buffer) = &working {
                        subscriptions.push(cx.subscribe(buffer, |_, _, event, cx| {
                            match event {
                                BufferEvent::Edited { .. } => cx.emit(EditorEvent::BufferEdited),
                                BufferEvent::Saved => cx.emit(EditorEvent::Saved),
                                BufferEvent::DirtyChanged => cx.emit(EditorEvent::DirtyChanged),
                                _ => {}
                            }
                            cx.notify();
                        }));
                    }
                    let mut marker_set = None;
                    let comparisons = comparisons
                        .into_iter()
                        .map(|(section, old, new, diff)| {
                            let labels = if section == Section::Text {
                                (t!("Mine"), t!("Merge Result"))
                            } else {
                                (t!("Mine"), t!("Incoming"))
                            };
                            let buffer = new.clone();
                            let view = cx.new(|cx| {
                                FileDiffView::new(
                                    old,
                                    new,
                                    diff,
                                    project.clone(),
                                    workbench.clone(),
                                    window,
                                    cx,
                                )
                                .with_snapshot_labels(
                                    title.clone(),
                                    labels.0.to_string(),
                                    labels.1.to_string(),
                                    cx,
                                )
                            });
                            if section == Section::Text {
                                let editor = view.read(cx).rhs_editor(cx);
                                let set = editor.update(cx, |editor, cx| {
                                    editor.disable_diagnostics(cx);
                                    git_ui::attach_conflict_markers(
                                        editor,
                                        buffer,
                                        t!("Mine"),
                                        t!("Incoming"),
                                        cx,
                                    )
                                });
                                subscriptions.push(cx.subscribe(&set, |_, _, _, cx| cx.notify()));
                                marker_set = Some(set);
                            }
                            (section, view)
                        })
                        .collect();
                    Conflict {
                        title: title.into(),
                        section: sections[0],
                        sections,
                        comparisons,
                        working,
                        marker_set,
                        details,
                        report,
                        is_mixed,
                        path,
                        root,
                        panel,
                        saving: None,
                        save_error: None,
                        _subscriptions: subscriptions,
                    }
                });
                scope.open(workspace, item, window, cx);
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
}

struct Conflict {
    title: SharedString,
    section: Section,
    sections: Vec<Section>,
    comparisons: Vec<(Section, Entity<FileDiffView>)>,
    working: Option<Entity<Buffer>>,
    marker_set: Option<Entity<project::ConflictSet>>,
    details: Entity<Editor>,
    report: Report,
    is_mixed: bool,
    path: String,
    root: PathBuf,
    panel: WeakEntity<SvnPanel>,
    saving: Option<Task<()>>,
    save_error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl Conflict {
    fn diff(&self) -> Option<&Entity<FileDiffView>> {
        self.comparisons
            .iter()
            .find(|(section, _)| *section == self.section)
            .map(|(_, view)| view)
    }

    fn editor(&self, cx: &App) -> Entity<Editor> {
        self.diff()
            .map_or_else(|| self.details.clone(), |diff| diff.read(cx).rhs_editor(cx))
    }

    fn marker_count(&self, cx: &App) -> usize {
        self.marker_set
            .as_ref()
            .map_or(0, |set| set.read(cx).snapshot.conflicts.len())
    }

    fn panel_action(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        action: impl FnOnce(&mut SvnPanel, String, &mut Window, &mut Context<SvnPanel>) + 'static,
    ) {
        let panel = self.panel.clone();
        let root = self.root.clone();
        let path = self.path.clone();
        // SVN writes inspect open items for unsaved files. Release this item's
        // entity before allowing the panel to read it back.
        window.defer(cx, move |window, cx| {
            let Some(panel) = panel.upgrade() else {
                return;
            };
            panel.update(cx, |panel, cx| {
                if panel.busy || !panel.repos.iter().any(|repo| repo.copy.root == root) {
                    return;
                }
                if panel.selected_root.as_ref() != Some(&root) {
                    panel.set_current(Some(root), window, cx);
                }
                action(panel, path, window, cx);
            });
        });
    }

    fn resolve(
        &mut self,
        accept: &'static str,
        label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kind = if self.is_mixed {
            "mixed"
        } else {
            match self.sections[0] {
                Section::Text => "text",
                Section::Properties => "property",
                Section::Tree => "tree",
                Section::Details => return,
            }
        };
        let expected_working = (kind == "tree" && accept == "theirs-full")
            .then(|| self.report.views.get("working"))
            .flatten();
        let params = json!({
            "operation": "resolve",
            "path": self.path,
            "accept": accept,
            "kind": kind,
            "incoming_revision": self.report.tree_incoming_revision,
            "expected_working": expected_working,
        });
        if !matches!(kind, "text" | "mixed") || accept != "working" {
            let mut detail = self.root.join(&self.path).display().to_string();
            if kind == "property" {
                let key = match accept {
                    "mine-full" => "property_mine",
                    "theirs-full" => "property_incoming",
                    "base" => "property_base",
                    _ => "",
                };
                if let Some(values) = self.report.views.get(key) {
                    detail.push_str("\n\n");
                    detail.push_str(&util::truncate_and_trailoff(values, 400));
                }
            }
            self.panel_action(window, cx, move |panel, _, window, cx| {
                panel.confirm(label, detail, "write", params, Output::Change, window, cx);
            });
            return;
        }
        if self.marker_count(cx) != 0 || self.saving.is_some() {
            return;
        }
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let project = panel.read(cx).project.clone();
        let save = self
            .working
            .as_ref()
            .filter(|buffer| buffer.read(cx).is_dirty())
            .map(|buffer| {
                project.update(cx, |project, cx| project.save_buffer(buffer.clone(), cx))
            });
        self.save_error = None;
        self.saving = Some(cx.spawn_in(window, async move |this, cx| {
            let saved = match save {
                Some(task) => task.await,
                None => Ok(()),
            };
            this.update_in(cx, |this, window, cx| {
                this.saving = None;
                if let Err(error) = saved {
                    this.save_error = Some(error.to_string());
                } else if !this.is_dirty(cx) && this.marker_count(cx) == 0 {
                    let confirmation = (kind == "mixed").then(|| {
                        let sections = this
                            .sections
                            .iter()
                            .copied()
                            .filter(|section| *section != Section::Details)
                            .map(|section| section.label().to_string())
                            .collect::<Vec<_>>()
                            .join(" · ");
                        format!("{}\n{sections}", this.root.join(&this.path).display())
                    });
                    this.panel_action(window, cx, move |panel, _, window, cx| {
                        if let Some(detail) = confirmation {
                            panel.confirm(
                                label,
                                detail,
                                "write",
                                params,
                                Output::Change,
                                window,
                                cx,
                            );
                        } else {
                            panel.run("write", params, Output::Change, window, cx);
                        }
                    });
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn resolution_button(
        &self,
        accept: &'static str,
        label: SharedString,
        disabled: bool,
        primary: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(accept, label.clone())
            .size(ButtonSize::Compact)
            .disabled(disabled)
            .when(primary, |button| button.style(ButtonStyle::Filled))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.resolve(accept, label.clone(), window, cx);
            }))
    }
}

impl EventEmitter<EditorEvent> for Conflict {}
impl Focusable for Conflict {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor(cx).focus_handle(cx)
    }
}

impl Item for Conflict {
    type Event = EditorEvent;

    fn to_item_events(event: &EditorEvent, f: &mut dyn FnMut(ItemEvent)) {
        Editor::to_item_events(event, f)
    }

    fn tab_content_text(&self, _: usize, _: &App) -> SharedString {
        self.title.clone()
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<Icon> {
        Some(Icon::new(IconName::Warning).color(Color::VersionControlConflict))
    }

    fn set_nav_history(&mut self, history: ItemNavHistory, _: &mut Window, cx: &mut Context<Self>) {
        self.editor(cx)
            .update(cx, |editor, _| editor.set_nav_history(Some(history)));
    }

    fn deactivated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor(cx)
            .update(cx, |editor, cx| editor.deactivated(window, cx));
    }

    fn navigate(
        &mut self,
        data: Arc<dyn Any + Send>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.editor(cx)
            .update(cx, |editor, cx| editor.navigate(data, window, cx))
    }

    fn active_project_path(&self, cx: &App) -> Option<project::ProjectPath> {
        self.working
            .as_ref()
            .and_then(|buffer| project::ProjectItem::project_path(buffer.read(cx), cx))
    }

    fn for_each_project_item(
        &self,
        cx: &App,
        f: &mut dyn FnMut(EntityId, &dyn project::ProjectItem),
    ) {
        if let Some(buffer) = &self.working {
            f(buffer.entity_id(), buffer.read(cx));
        }
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.working
            .as_ref()
            .is_some_and(|buffer| buffer.read(cx).is_dirty())
    }

    fn can_save(&self, _: &App) -> bool {
        self.working.is_some()
    }

    fn save(
        &mut self,
        _: SaveOptions,
        project: Entity<project::Project>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let Some(buffer) = &self.working else {
            return Task::ready(Ok(()));
        };
        project.update(cx, |project, cx| project.save_buffer(buffer.clone(), cx))
    }

    fn reload(
        &mut self,
        project: Entity<project::Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        let editor = self.editor(cx);
        editor.update(cx, |editor, cx| editor.reload(project, window, cx))
    }

    fn act_as_type<'a>(
        &'a self,
        type_id: TypeId,
        handle: &'a Entity<Self>,
        cx: &'a App,
    ) -> Option<AnyEntity> {
        if type_id == TypeId::of::<Self>() {
            Some(handle.clone().into())
        } else if type_id == TypeId::of::<Editor>() {
            Some(self.editor(cx).into())
        } else {
            None
        }
    }

    fn as_searchable(&self, _: &Entity<Self>, cx: &App) -> Option<Box<dyn SearchableItemHandle>> {
        Some(Box::new(self.editor(cx)))
    }
}

impl Render for Conflict {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (active, busy) = self
            .panel
            .read_with(cx, |panel, _| {
                let active = panel.repos.iter().any(|repo| {
                    repo.copy.root == self.root
                        && repo
                            .entries
                            .iter()
                            .any(|entry| entry.path == self.path && entry.conflicted())
                });
                (active, panel.busy)
            })
            .unwrap_or_default();
        let disabled = !active || busy || self.saving.is_some();
        let markers = if self.section == Section::Text {
            self.marker_count(cx)
        } else {
            0
        };
        let diff = self.diff().cloned();
        let controls = diff.as_ref().and_then(|diff| {
            let inner = diff.read(cx);
            inner
                .act_as_type(TypeId::of::<SplittableEditor>(), diff, cx)?
                .downcast::<SplittableEditor>()
                .ok()
        });
        let body: AnyElement = if let Some(diff) = diff {
            diff.into_any_element()
        } else {
            self.details.clone().into_any_element()
        };
        let editor = self.editor(cx);
        let finish = self.sections[0] == self.section
            && (self.working.is_some() || self.section != Section::Text);
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .min_h_8()
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().colors().border_variant)
                    .when_some(controls.clone(), |bar, controls| {
                        bar.child(editor::DiffStyleControls::new(controls))
                    })
                    .children(self.sections.iter().copied().map(|section| {
                        Button::new(section.label(), section.label())
                            .size(ButtonSize::Compact)
                            .when(section == self.section, |button| {
                                button.style(ButtonStyle::Tinted(ui::TintColor::Accent))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.section = section;
                                cx.notify();
                            }))
                    }))
                    .child(div().flex_1())
                    .when(
                        active && self.section == Section::Text && self.working.is_some(),
                        |bar| {
                            bar.child(
                                Label::new(t!(
                                    "Remaining Conflict Blocks: {count}",
                                    count = markers
                                ))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                            )
                        },
                    )
                    .when(controls.is_some(), |bar| {
                        bar.children(
                            [
                                (
                                    "previous",
                                    IconName::ArrowUp,
                                    t!("Previous Hunk"),
                                    "editor::GoToPreviousHunk",
                                ),
                                (
                                    "next",
                                    IconName::ArrowDown,
                                    t!("Next Hunk"),
                                    "editor::GoToHunk",
                                ),
                            ]
                            .map(|(id, icon, label, action)| {
                                let editor = editor.clone();
                                IconButton::new(id, icon)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(label))
                                    .on_click(move |_, window, cx| {
                                        editor.focus_handle(cx).focus(window, cx);
                                        if let Ok(action) = cx.build_action(action, None) {
                                            window.dispatch_action(action, cx);
                                        }
                                    })
                            }),
                        )
                    })
                    .when(
                        active && !self.is_mixed && self.section == Section::Properties,
                        |bar| {
                            bar.child(self.resolution_button(
                                "mine-full",
                                t!("Use Local Properties"),
                                disabled,
                                false,
                                cx,
                            ))
                            .child(self.resolution_button(
                                "theirs-full",
                                t!("Use Incoming Properties"),
                                disabled,
                                false,
                                cx,
                            ))
                            .child(self.resolution_button(
                                "base",
                                t!("Use Base Properties"),
                                disabled,
                                false,
                                cx,
                            ))
                        },
                    )
                    .when(active && self.section == Section::Properties, |bar| {
                        bar.child(
                            Button::new("edit-property", t!("Set SVN Property"))
                                .size(ButtonSize::Compact)
                                .disabled(disabled)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.panel_action(window, cx, |panel, path, window, cx| {
                                        panel.start_form_for(
                                            form::Kind::SetProperty,
                                            path,
                                            true,
                                            window,
                                            cx,
                                        );
                                    });
                                })),
                        )
                    })
                    .when(
                        active
                            && self.section == Section::Tree
                            && self.report.tree_incoming_revision.is_some(),
                        |bar| {
                            bar.child(self.resolution_button(
                                "theirs-full",
                                t!("Use Incoming File"),
                                disabled,
                                false,
                                cx,
                            ))
                        },
                    )
                    .when(active && finish, |bar| {
                        let label = if !self.is_mixed && self.section == Section::Tree {
                            t!("Keep Working Copy")
                        } else {
                            t!("Mark Resolved")
                        };
                        bar.child(self.resolution_button(
                            "working",
                            label,
                            disabled || markers != 0,
                            true,
                            cx,
                        ))
                    })
                    .when(
                        active
                            && !self.is_mixed
                            && self.section == Section::Details
                            && self.sections.first() == Some(&Section::Text),
                        |bar| {
                            bar.child(self.resolution_button(
                                "mine-full",
                                t!("Use {branch}", branch = t!("Mine")),
                                disabled,
                                false,
                                cx,
                            ))
                            .child(self.resolution_button(
                                "theirs-full",
                                t!("Use Incoming File"),
                                disabled,
                                false,
                                cx,
                            ))
                            .child(self.resolution_button(
                                "base",
                                t!("Use {branch}", branch = t!("Base")),
                                disabled,
                                false,
                                cx,
                            ))
                        },
                    )
                    .when(
                        active && matches!(self.section, Section::Details | Section::Tree),
                        |bar| {
                            bar.child(
                                Button::new("terminal-resolve", t!("Open in Terminal"))
                                    .size(ButtonSize::Compact)
                                    .disabled(disabled)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.panel_action(window, cx, |panel, path, window, cx| {
                                            panel.interactive_resolve(path, window, cx);
                                        });
                                    })),
                            )
                        },
                    )
                    .when(!active, |bar| {
                        bar.child(
                            Label::new(t!("Conflict Resolved"))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    }),
            )
            .when(self.section == Section::Tree, |view| {
                view.when_some(self.report.tree.as_ref(), |view, tree| {
                    let summary = if tree.operation == "update"
                        && tree.reason == "add"
                        && tree.action == "add"
                    {
                        t!("Both sides added this path during update.")
                    } else {
                        format!("{} · {} ↔ {}", tree.operation, tree.reason, tree.action).into()
                    };
                    view.child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .gap_2()
                            .child(Icon::new(IconName::Warning).color(Color::Warning))
                            .child(Label::new(summary).color(Color::Muted)),
                    )
                })
            })
            .when_some(self.save_error.clone().filter(|_| active), |view, error| {
                view.child(
                    div()
                        .px_2()
                        .py_1()
                        .child(Label::new(error).color(Color::Error)),
                )
            })
            .child(div().flex_1().min_h_0().child(body))
    }
}
