use super::*;
use ui::ListItemSpacing;

#[derive(Deserialize)]
pub(super) struct Target {
    path: String,
    url: String,
}

#[derive(Deserialize)]
pub(super) struct Targets {
    pub(super) targets: Vec<Target>,
}

pub(super) struct BranchPicker {
    picker: Entity<picker::Picker<Delegate>>,
}

struct Delegate {
    parent: WeakEntity<BranchPicker>,
    panel: WeakEntity<SvnPanel>,
    root: PathBuf,
    current_url: String,
    targets: Vec<Target>,
    matches: Vec<usize>,
    selected: usize,
}

impl BranchPicker {
    pub(super) fn new(
        panel: WeakEntity<SvnPanel>,
        copy: Copy,
        targets: Vec<Target>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = Delegate {
            parent: cx.weak_entity(),
            panel,
            root: copy.root,
            current_url: copy.url,
            matches: (0..targets.len()).collect(),
            targets,
            selected: 0,
        };
        let picker = cx.new(|cx| {
            picker::Picker::uniform_list(delegate, window, cx)
                .initial_width(rems(20.))
                .show_scrollbar(true)
        });
        Self { picker }
    }
}

impl workspace::ModalView for BranchPicker {}

impl EventEmitter<gpui::DismissEvent> for BranchPicker {}

impl Focusable for BranchPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for BranchPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().elevation_3(cx).child(self.picker.clone())
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
        Some(t!("No Results"))
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
