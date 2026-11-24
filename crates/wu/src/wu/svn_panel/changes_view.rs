use super::*;
use gpui::Anchor;
use serde::Serialize;
use std::path::Path;
use ui::{ContextMenu, ContextMenuEntry, IconPosition, PopoverMenu};

const VIEW_OPTIONS_KEY: &str = "svn-panel-view-options";

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct ViewOptions {
    tree: bool,
    sort: Sort,
    group: Group,
    pub show_ignored: bool,
    pub history_graph: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Sort {
    #[default]
    Path,
    Name,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Group {
    None,
    #[default]
    Status,
    Selection,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Section {
    Conflicts,
    All,
    Tracked,
    Untracked,
    Checked,
    Unchecked,
    Ignored,
    External,
}

impl Section {
    const ORDER: [Self; 8] = [
        Self::Conflicts,
        Self::All,
        Self::Tracked,
        Self::Untracked,
        Self::Checked,
        Self::Unchecked,
        Self::Ignored,
        Self::External,
    ];

    pub fn label(self) -> Option<SharedString> {
        Some(match self {
            Self::Conflicts => t!("Conflicts"),
            Self::All => return None,
            Self::Tracked => t!("Tracked"),
            Self::Untracked => t!("Untracked"),
            Self::Checked => t!("Checked for Commit"),
            Self::Unchecked => t!("Unchecked Changes"),
            Self::Ignored => t!("Ignored"),
            Self::External => t!("External"),
        })
    }

    pub fn test_id(self) -> &'static str {
        match self {
            Self::Conflicts => "conflicts",
            Self::All => "all",
            Self::Tracked => "tracked",
            Self::Untracked => "untracked",
            Self::Checked => "checked",
            Self::Unchecked => "unchecked",
            Self::Ignored => "ignored",
            Self::External => "external",
        }
    }
}

type Changes<'a> = Vec<&'a Change>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Row<'a> {
    Change(&'a str),
    Directory(Section, &'a str),
}

pub(super) enum Entry<'a> {
    Change {
        change: &'a Change,
        depth: usize,
    },
    Directory {
        path: String,
        name: &'a str,
        depth: usize,
        change: Option<&'a Change>,
        paths: Vec<String>,
        collapsed: bool,
    },
}

impl Entry<'_> {
    pub fn row(&self, section: Section) -> Row<'_> {
        match self {
            Self::Change { change, .. } => Row::Change(&change.path),
            Self::Directory { path, .. } => Row::Directory(section, path),
        }
    }
}

pub(super) struct ChangeGroup<'a> {
    pub section: Section,
    pub paths: Vec<String>,
    pub rows: Vec<Entry<'a>>,
}

#[derive(Default)]
struct Directory<'a> {
    change: Option<&'a Change>,
    children: BTreeMap<&'a str, Directory<'a>>,
    files: Changes<'a>,
}

impl<'a> Directory<'a> {
    fn insert(&mut self, change: &'a Change) {
        let mut parts = change.path.split('/').peekable();
        let mut directory = self;
        while let Some(part) = parts.next() {
            if parts.peek().is_some() || change.is_dir && part != "." {
                directory = directory.children.entry(part).or_default();
            } else {
                directory.files.push(change);
                return;
            }
        }
        directory.change = Some(change);
    }

    fn committable_paths(&self, paths: &mut Vec<String>) {
        paths.extend(
            self.change
                .into_iter()
                .chain(self.files.iter().copied())
                .filter(|entry| entry.committable())
                .map(|entry| entry.path.clone()),
        );
        for child in self.children.values() {
            child.committable_paths(paths);
        }
    }

    fn rows(
        self,
        section: Section,
        parent: &str,
        depth: usize,
        collapsed_directories: &BTreeSet<(Section, String)>,
        output: &mut Vec<Entry<'a>>,
    ) {
        for (name, folder) in self.children {
            if folder.children.is_empty()
                && folder.files.is_empty()
                && let Some(change) = folder.change
            {
                output.push(Entry::Change { change, depth });
                continue;
            }
            let path = if parent.is_empty() {
                name.to_owned()
            } else {
                format!("{parent}/{name}")
            };
            let collapsed = collapsed_directories.contains(&(section, path.clone()));
            let mut paths = Vec::new();
            folder.committable_paths(&mut paths);
            output.push(Entry::Directory {
                path: path.clone(),
                name,
                depth,
                change: folder.change,
                paths,
                collapsed,
            });
            if !collapsed {
                folder.rows(section, &path, depth + 1, collapsed_directories, output);
            }
        }
        output.extend(
            self.files
                .into_iter()
                .map(|change| Entry::Change { change, depth }),
        );
    }
}

impl ViewOptions {
    pub fn is_tree(self) -> bool {
        self.tree
    }

    pub fn load(cx: &App) -> Self {
        db::kvp::KeyValueStore::global(cx)
            .read_kvp(VIEW_OPTIONS_KEY)
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default()
    }

    fn section(self, entry: &Change, checked: &BTreeSet<String>) -> Section {
        if entry.conflicted() {
            return Section::Conflicts;
        }
        match (entry.item.as_str(), self.group) {
            ("ignored", _) => Section::Ignored,
            ("external", _) => Section::External,
            (_, Group::None) => Section::All,
            ("unversioned", Group::Status) => Section::Untracked,
            (_, Group::Status) => Section::Tracked,
            (_, Group::Selection) if entry.committable() && checked.contains(&entry.path) => {
                Section::Checked
            }
            (_, Group::Selection) => Section::Unchecked,
        }
    }

    fn menu(self, menu: ContextMenu, panel: WeakEntity<SvnPanel>) -> ContextMenu {
        let item = |label, enabled, next| {
            let panel = panel.clone();
            ContextMenuEntry::new(label)
                .toggle(IconPosition::End, enabled)
                .handler(move |window, cx| {
                    let _ =
                        panel.update(cx, |panel, cx| panel.apply_view_options(next, window, cx));
                })
        };
        menu.header(t!("View"))
            .item(item(
                t!("List"),
                !self.tree,
                Self {
                    tree: false,
                    ..self
                },
            ))
            .item(item(t!("Tree"), self.tree, Self { tree: true, ..self }))
            .when(!self.tree, |menu| {
                menu.separator()
                    .header(t!("Sort By"))
                    .item(item(
                        t!("Path"),
                        self.sort == Sort::Path,
                        Self {
                            sort: Sort::Path,
                            ..self
                        },
                    ))
                    .item(item(
                        t!("Name"),
                        self.sort == Sort::Name,
                        Self {
                            sort: Sort::Name,
                            ..self
                        },
                    ))
            })
            .separator()
            .header(t!("Group By"))
            .item(item(
                t!("None"),
                self.group == Group::None,
                Self {
                    group: Group::None,
                    ..self
                },
            ))
            .item(item(
                t!("Tracked & Untracked"),
                self.group == Group::Status,
                Self {
                    group: Group::Status,
                    ..self
                },
            ))
            .item(item(
                t!("Checked & Unchecked"),
                self.group == Group::Selection,
                Self {
                    group: Group::Selection,
                    ..self
                },
            ))
            .separator()
            .item(item(
                t!("Show Ignored Files"),
                self.show_ignored,
                Self {
                    show_ignored: !self.show_ignored,
                    ..self
                },
            ))
    }
}

impl SvnPanel {
    pub(super) fn view_options_menu(&self, cx: &Context<Self>) -> AnyElement {
        let panel = cx.weak_entity();
        let focus = self.focus.clone();
        PopoverMenu::new("svn-view-options")
            .trigger_with_tooltip(
                IconButton::new("svn-view-options-trigger", IconName::Filter)
                    .icon_size(IconSize::Small),
                Tooltip::text(t!("View Options")),
            )
            .menu(move |window, cx| {
                let (panel, focus) = (panel.clone(), focus.clone());
                Some(ContextMenu::build_persistent(
                    window,
                    cx,
                    move |menu, _, cx| {
                        let options = panel
                            .read_with(cx, |panel, _| panel.view_options)
                            .unwrap_or_default();
                        options.menu(menu.context(focus.clone()), panel.clone())
                    },
                ))
            })
            .anchor(Anchor::TopRight)
            .into_any_element()
    }

    pub(super) fn history_options_menu(&self, cx: &Context<Self>) -> AnyElement {
        let panel = cx.weak_entity();
        let focus = self.focus.clone();
        PopoverMenu::new("svn-history-options")
            .trigger_with_tooltip(
                IconButton::new("svn-history-options-trigger", IconName::Filter)
                    .icon_size(IconSize::Small),
                Tooltip::text(t!("View Options")),
            )
            .menu(move |window, cx| {
                let (panel, focus) = (panel.clone(), focus.clone());
                Some(ContextMenu::build_persistent(
                    window,
                    cx,
                    move |menu, _, cx| {
                        let options = panel
                            .read_with(cx, |panel, _| panel.view_options)
                            .unwrap_or_default();
                        [(t!("Details"), false), (t!("Graph"), true)]
                            .into_iter()
                            .fold(
                                menu.context(focus.clone()).header(t!("View")),
                                |menu, (label, graph)| {
                                    let panel = panel.clone();
                                    menu.item(
                                        ContextMenuEntry::new(label)
                                            .toggle(
                                                IconPosition::End,
                                                options.history_graph == graph,
                                            )
                                            .handler(move |window, cx| {
                                                let _ = panel.update(cx, |panel, cx| {
                                                    panel.apply_view_options(
                                                        ViewOptions {
                                                            history_graph: graph,
                                                            ..panel.view_options
                                                        },
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            }),
                                    )
                                },
                            )
                    },
                ))
            })
            .anchor(Anchor::TopRight)
            .into_any_element()
    }

    fn apply_view_options(
        &mut self,
        options: ViewOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous = self.view_options;
        if previous == options {
            return;
        }
        self.view_options = options;
        if previous.history_graph != options.history_graph {
            self.history.remeasure();
        }
        if previous.group != options.group || previous.tree != options.tree {
            self.active_directory = None;
            self.scroll.set_offset(gpui::point(px(0.), px(0.)));
        }
        let db = db::kvp::KeyValueStore::global(cx);
        if let Ok(value) = serde_json::to_string(&options) {
            let prior = self.save_view_options.take();
            self.save_view_options = Some(cx.background_spawn(async move {
                if let Some(prior) = prior {
                    prior.await;
                }
                if let Err(error) = db.write_kvp(VIEW_OPTIONS_KEY.to_owned(), value).await {
                    log::error!("Could not save SVN view options: {error}");
                }
            }));
        }
        if previous.show_ignored != options.show_ignored {
            self.refresh(false, window, cx);
        }
        cx.notify();
    }

    pub(super) fn change_groups(&self) -> Vec<ChangeGroup<'_>> {
        let Some(repo) = self.repo() else {
            return Vec::new();
        };
        let options = self.view_options;
        let mut grouped: BTreeMap<Section, Changes<'_>> = BTreeMap::new();
        for entry in &repo.entries {
            if entry.item != "ignored" || options.show_ignored {
                grouped
                    .entry(options.section(entry, &self.selected_paths))
                    .or_default()
                    .push(entry);
            }
        }

        Section::ORDER
            .into_iter()
            .filter_map(|section| {
                let mut entries = grouped.remove(&section)?;
                let paths = entries
                    .iter()
                    .filter(|entry| entry.committable())
                    .map(|entry| entry.path.clone())
                    .collect();
                let rows = if self.collapsed_groups.contains(&section) {
                    Vec::new()
                } else if options.tree {
                    let mut tree = Directory::default();
                    for entry in entries {
                        tree.insert(entry);
                    }
                    let mut rows = Vec::new();
                    tree.rows(section, "", 0, &self.collapsed_directories, &mut rows);
                    rows
                } else {
                    if options.sort == Sort::Name {
                        entries.sort_by(|left, right| {
                            Path::new(&left.path)
                                .file_name()
                                .cmp(&Path::new(&right.path).file_name())
                                .then_with(|| left.path.cmp(&right.path))
                        });
                    }
                    entries
                        .into_iter()
                        .map(|change| Entry::Change { change, depth: 0 })
                        .collect()
                };
                Some(ChangeGroup {
                    section,
                    paths,
                    rows,
                })
            })
            .collect()
    }
}
