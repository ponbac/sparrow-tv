//! Logical focus movement in the television Now grid.

use crate::{
    BoardView,
    board_focus::{DownSteps, RowMove},
};

const COLUMNS: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// A Channel, one of its Quality Variant controls, or the retained retry control.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NowFocus {
    Channel(usize),
    Variant { family: usize, chip: usize },
    Retry,
}

impl NowFocus {
    pub fn family(self) -> Option<usize> {
        match self {
            Self::Channel(family) | Self::Variant { family, .. } => Some(family),
            Self::Retry => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NowMove {
    Stay,
    Focus(NowFocus),
    ToGroupLane,
    /// Request the continuation and keep the current focus while it loads.
    LoadMore,
}

#[derive(Default)]
pub struct NowNavigation {
    down: DownSteps<COLUMNS, NowFocus, ()>,
}

impl NowNavigation {
    /// Moves over loaded families by index, independent of the rendered viewport.
    /// `variant_count` is the visible Quality Variant control count in the current family.
    pub fn press(
        &mut self,
        focus: NowFocus,
        direction: Direction,
        variant_count: usize,
        board: BoardView,
    ) -> NowMove {
        if direction != Direction::Down {
            self.down.clear();
        }
        let Some(family) = focus.family() else {
            self.down.clear();
            return match (direction, board.families.checked_sub(1)) {
                (Direction::Up, Some(last)) => NowMove::Focus(NowFocus::Channel(last)),
                (Direction::Up, None) => NowMove::ToGroupLane,
                _ => NowMove::Stay,
            };
        };
        if family >= board.families
            || matches!(focus, NowFocus::Variant { chip, .. } if chip >= variant_count)
        {
            self.down.clear();
            return NowMove::Stay;
        }

        let column = family % COLUMNS;
        match direction {
            Direction::Up if family < COLUMNS => NowMove::ToGroupLane,
            Direction::Up => NowMove::Focus(NowFocus::Channel(family - COLUMNS)),
            Direction::Left => match focus {
                NowFocus::Variant { chip: 0, .. } => NowMove::Focus(NowFocus::Channel(family)),
                NowFocus::Variant { chip, .. } => NowMove::Focus(NowFocus::Variant {
                    family,
                    chip: chip - 1,
                }),
                _ if column > 0 => NowMove::Focus(NowFocus::Channel(family - 1)),
                _ => NowMove::Stay,
            },
            Direction::Right => {
                let next_chip = match focus {
                    NowFocus::Variant { chip, .. } => chip + 1,
                    _ => 0,
                };
                if next_chip < variant_count {
                    NowMove::Focus(NowFocus::Variant {
                        family,
                        chip: next_chip,
                    })
                } else if column + 1 < COLUMNS && family + 1 < board.families {
                    NowMove::Focus(NowFocus::Channel(family + 1))
                } else {
                    NowMove::Stay
                }
            }
            Direction::Down => movement(self.down.down(focus, family, (), board)),
        }
    }

    /// Rechecks a deferred Down after a page or availability change.
    pub fn settle(&mut self, focus: Option<NowFocus>, board: BoardView) -> NowMove {
        movement(self.down.settle(focus, board))
    }

    /// Restores board focus after the previously focused control was replaced.
    pub fn recover(&mut self, board: BoardView) -> NowMove {
        self.down.clear();
        movement(board.fallback())
    }
}

fn movement(destination: RowMove<()>) -> NowMove {
    match destination {
        RowMove::Stay => NowMove::Stay,
        RowMove::Focus(family, ()) => NowMove::Focus(NowFocus::Channel(family)),
        RowMove::Retry => NowMove::Focus(NowFocus::Retry),
        RowMove::LoadMore => NowMove::LoadMore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PageStatus;

    fn board(families: usize, page: PageStatus) -> BoardView {
        BoardView {
            selection: 0,
            families,
            page,
        }
    }

    #[test]
    fn arrows_use_grid_columns_and_fall_back_to_the_last_cell_of_a_partial_line() {
        let mut navigation = NowNavigation::default();
        assert_eq!(
            navigation.press(
                NowFocus::Channel(2),
                Direction::Right,
                0,
                board(4, PageStatus::Complete),
            ),
            NowMove::Stay,
        );
        assert_eq!(
            navigation.press(
                NowFocus::Channel(2),
                Direction::Down,
                0,
                board(4, PageStatus::Complete),
            ),
            NowMove::Focus(NowFocus::Channel(3)),
        );
        assert_eq!(
            navigation.press(
                NowFocus::Channel(3),
                Direction::Left,
                0,
                board(4, PageStatus::Complete),
            ),
            NowMove::Stay,
        );
        assert_eq!(
            navigation.press(
                NowFocus::Channel(3),
                Direction::Up,
                0,
                board(4, PageStatus::Complete)
            ),
            NowMove::Focus(NowFocus::Channel(0)),
        );
        assert_eq!(
            navigation.press(
                NowFocus::Channel(0),
                Direction::Up,
                0,
                board(4, PageStatus::Complete)
            ),
            NowMove::ToGroupLane,
        );
    }

    #[test]
    fn right_visits_own_quality_controls_and_vertical_navigation_returns_to_channels() {
        let mut navigation = NowNavigation::default();
        let channel = NowFocus::Channel(1);
        let sd = NowFocus::Variant { family: 1, chip: 0 };
        let hd = NowFocus::Variant { family: 1, chip: 1 };
        let mut press = |focus, direction| {
            navigation.press(focus, direction, 2, board(4, PageStatus::Complete))
        };
        assert_eq!(press(channel, Direction::Right), NowMove::Focus(sd));
        assert_eq!(press(sd, Direction::Right), NowMove::Focus(hd));
        assert_eq!(
            press(hd, Direction::Right),
            NowMove::Focus(NowFocus::Channel(2))
        );
        assert_eq!(
            press(NowFocus::Channel(2), Direction::Left),
            NowMove::Focus(channel)
        );
        assert_eq!(press(hd, Direction::Left), NowMove::Focus(sd));
        assert_eq!(press(sd, Direction::Left), NowMove::Focus(channel));
        assert_eq!(
            press(sd, Direction::Down),
            NowMove::Focus(NowFocus::Channel(3))
        );
    }

    #[test]
    fn deferred_down_continues_through_a_page_that_only_adds_variants() {
        let mut navigation = NowNavigation::default();
        let origin = NowFocus::Channel(1);
        assert_eq!(
            navigation.press(origin, Direction::Down, 0, board(4, PageStatus::Ready)),
            NowMove::LoadMore
        );
        assert_eq!(
            navigation.settle(Some(origin), board(4, PageStatus::Loading)),
            NowMove::Stay
        );
        assert_eq!(
            navigation.settle(Some(origin), board(4, PageStatus::Ready)),
            NowMove::LoadMore
        );
        assert_eq!(
            navigation.settle(Some(origin), board(5, PageStatus::Ready)),
            NowMove::Focus(NowFocus::Channel(4))
        );
        assert_eq!(
            navigation.settle(Some(origin), board(5, PageStatus::Ready)),
            NowMove::Stay
        );
    }

    #[test]
    fn held_down_is_capped_at_eight_unshown_steps() {
        let mut navigation = NowNavigation::default();
        let origin = NowFocus::Channel(1);
        for _ in 0..30 {
            assert_eq!(
                navigation.press(origin, Direction::Down, 0, board(3, PageStatus::Loading)),
                NowMove::Stay
            );
        }
        assert_eq!(
            navigation.settle(Some(origin), board(26, PageStatus::Ready)),
            NowMove::Focus(NowFocus::Channel(25))
        );
    }

    #[test]
    fn final_page_resolves_deferred_down_or_moves_focus_to_retry_on_failure() {
        let mut navigation = NowNavigation::default();
        let origin = NowFocus::Channel(2);
        navigation.press(origin, Direction::Down, 0, board(3, PageStatus::Loading));
        assert_eq!(
            navigation.settle(Some(origin), board(4, PageStatus::Complete)),
            NowMove::Focus(NowFocus::Channel(3))
        );
        navigation.press(origin, Direction::Down, 0, board(3, PageStatus::Loading));
        assert_eq!(
            navigation.settle(Some(origin), board(3, PageStatus::Failed)),
            NowMove::Focus(NowFocus::Retry)
        );
        assert_eq!(
            navigation.press(origin, Direction::Down, 0, board(4, PageStatus::Failed)),
            NowMove::Focus(NowFocus::Channel(3))
        );
        assert_eq!(
            navigation.press(origin, Direction::Down, 0, board(3, PageStatus::Failed)),
            NowMove::Focus(NowFocus::Retry)
        );
    }

    #[test]
    fn page_arrival_cannot_steal_focus_after_navigation_changes() {
        let mut navigation = NowNavigation::default();
        let origin = NowFocus::Channel(1);
        navigation.press(origin, Direction::Down, 0, board(3, PageStatus::Loading));
        assert_eq!(
            navigation.settle(Some(NowFocus::Channel(0)), board(6, PageStatus::Complete)),
            NowMove::Stay
        );
        assert_eq!(
            navigation.settle(Some(origin), board(6, PageStatus::Complete)),
            NowMove::Stay
        );

        navigation.press(origin, Direction::Down, 0, board(3, PageStatus::Loading));
        assert_eq!(
            navigation.press(origin, Direction::Up, 0, board(3, PageStatus::Loading)),
            NowMove::ToGroupLane
        );
        assert_eq!(
            navigation.settle(Some(origin), board(6, PageStatus::Complete)),
            NowMove::Stay
        );
    }

    #[test]
    fn selection_replacement_discards_old_motion_and_can_restore_board_focus() {
        let mut navigation = NowNavigation::default();
        let origin = NowFocus::Channel(0);
        assert_eq!(
            navigation.press(origin, Direction::Down, 0, board(3, PageStatus::Ready)),
            NowMove::LoadMore,
        );
        let replacement = BoardView {
            selection: 1,
            ..board(6, PageStatus::Complete)
        };
        assert_eq!(navigation.settle(Some(origin), replacement), NowMove::Stay);
        assert_eq!(navigation.recover(replacement), NowMove::Focus(origin));
        assert_eq!(
            navigation.recover(board(0, PageStatus::Failed)),
            NowMove::Focus(NowFocus::Retry),
        );
        assert_eq!(
            navigation.press(
                NowFocus::Retry,
                Direction::Up,
                0,
                board(0, PageStatus::Failed)
            ),
            NowMove::ToGroupLane,
        );
        assert_eq!(
            navigation.press(
                NowFocus::Retry,
                Direction::Up,
                0,
                board(4, PageStatus::Failed)
            ),
            NowMove::Focus(NowFocus::Channel(3)),
        );
    }
}
