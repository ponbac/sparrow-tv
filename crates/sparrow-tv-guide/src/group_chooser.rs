//! Channel Group chooser navigation and filtering, independent of the host editor.

use sparrow_core::{ChannelGroupFilter, ChannelGroupView};

use crate::now_focus::Direction;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupChooserFocus {
    Close,
    Filter,
    ShowAll,
    /// A filtered row, with row 0 reserved for All.
    Pick(usize),
    /// The Show or Hide button of a filtered group row.
    Toggle(usize),
}

impl GroupChooserFocus {
    pub fn row(self) -> Option<usize> {
        match self {
            Self::Pick(row) | Self::Toggle(row) => Some(row),
            _ => None,
        }
    }
}

pub enum GroupChooserIntent {
    Filter(String),
    Activate(GroupChooserFocus),
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupRosterCounts {
    pub shown: usize,
    pub hidden: usize,
}

pub struct GroupChooser {
    filter: String,
    matches: Vec<usize>,
    focus: GroupChooserFocus,
}

impl GroupChooser {
    pub(crate) fn new(groups: &[ChannelGroupView], selected: Option<&ChannelGroupFilter>) -> Self {
        let mut chooser = Self {
            filter: String::new(),
            matches: Vec::new(),
            focus: GroupChooserFocus::Pick(0),
        };
        chooser.rebuild_matches(groups);
        chooser.focus_selected(groups, selected);
        chooser
    }

    pub fn focus(&self) -> GroupChooserFocus {
        self.focus
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Indices into the full roster. The renderer places the All row before these matches.
    pub fn matches(&self) -> &[usize] {
        &self.matches
    }

    pub(crate) fn group_index(&self, row: usize) -> Option<usize> {
        self.matches.get(row.checked_sub(1)?).copied()
    }

    pub(crate) fn rebuild_matches(&mut self, groups: &[ChannelGroupView]) {
        let needle = self.filter.trim().to_lowercase();
        self.matches.clear();
        self.matches
            .extend(groups.iter().enumerate().filter_map(|(index, group)| {
                let label = if group.name().is_empty() {
                    "Ungrouped"
                } else {
                    group.name()
                };
                (needle.is_empty() || label.to_lowercase().contains(&needle)).then_some(index)
            }));
        if self.focus.row().is_some_and(|row| row > self.matches.len()) {
            self.focus = GroupChooserFocus::Pick(0);
        }
    }

    pub(crate) fn change_filter(&mut self, filter: String, groups: &[ChannelGroupView]) {
        self.filter = filter;
        self.focus = GroupChooserFocus::Filter;
        self.rebuild_matches(groups);
    }

    pub(crate) fn focus_selected(
        &mut self,
        groups: &[ChannelGroupView],
        selected: Option<&ChannelGroupFilter>,
    ) {
        self.focus = GroupChooserFocus::Pick(
            selected
                .and_then(|selected| {
                    self.matches
                        .iter()
                        .position(|index| groups[*index].name() == selected.as_str())
                })
                .map_or(0, |index| index + 1),
        );
    }

    pub(crate) fn reconcile_show_all(&mut self, can_show_all: bool) {
        if !can_show_all && self.focus == GroupChooserFocus::ShowAll {
            self.focus = GroupChooserFocus::Filter;
        }
    }

    pub(crate) fn focus_control(&mut self, target: GroupChooserFocus, can_show_all: bool) -> bool {
        let available = match target {
            GroupChooserFocus::ShowAll => can_show_all,
            GroupChooserFocus::Pick(row) => row <= self.matches.len(),
            GroupChooserFocus::Toggle(row) => self.group_index(row).is_some(),
            GroupChooserFocus::Close | GroupChooserFocus::Filter => true,
        };
        if available {
            self.focus = target;
        }
        available
    }

    pub(crate) fn arrow(&mut self, direction: Direction, can_show_all: bool) {
        use GroupChooserFocus::*;
        self.focus = match (self.focus, direction) {
            (Close, Direction::Down) => Filter,
            (Filter | ShowAll, Direction::Up) => Close,
            (Filter | ShowAll, Direction::Down) => Pick(0),
            (Filter, Direction::Right) if can_show_all => ShowAll,
            (ShowAll, Direction::Left) => Filter,
            (Pick(0), Direction::Up) => Filter,
            (Pick(row), Direction::Up) => Pick(row - 1),
            (Pick(row), Direction::Down) if row < self.matches.len() => Pick(row + 1),
            (Pick(row), Direction::Right) if row != 0 => Toggle(row),
            (Toggle(row), Direction::Left) => Pick(row),
            (Toggle(1), Direction::Up) if can_show_all => ShowAll,
            (Toggle(1), Direction::Up) => Filter,
            (Toggle(row), Direction::Up) => Toggle(row - 1),
            (Toggle(row), Direction::Down) if row < self.matches.len() => Toggle(row + 1),
            _ => self.focus,
        };
    }
}
