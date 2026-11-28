//! Movement between Timeline rows without losing the original Programme time.

use chrono::{DateTime, Utc};

use crate::{
    BoardView,
    board_focus::{DownSteps, RowMove},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimelineFocus {
    Channel(usize),
    Programme { family: usize, cell: usize },
    Variant { family: usize, chip: usize },
    Retry,
}

impl TimelineFocus {
    pub fn family(self) -> Option<usize> {
        match self {
            Self::Channel(family)
            | Self::Programme { family, .. }
            | Self::Variant { family, .. } => Some(family),
            Self::Retry => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimelineMove {
    Stay,
    Focus(TimelineFocus),
    /// Resolve the destination in the given family with Timeline's time-axis rules.
    Row {
        family: usize,
        anchor: DateTime<Utc>,
    },
    ToGroupLane,
    LoadMore,
}

#[derive(Default)]
pub struct TimelineNavigation {
    down: DownSteps<TimelineFocus, DateTime<Utc>>,
}

impl TimelineNavigation {
    pub fn down(
        &mut self,
        focus: TimelineFocus,
        anchor: DateTime<Utc>,
        board: BoardView,
    ) -> TimelineMove {
        let Some(family) = focus.family() else {
            self.cancel_pending();
            return TimelineMove::Stay;
        };
        movement(self.down.down(focus, family, anchor, board))
    }

    pub fn up(
        &mut self,
        focus: TimelineFocus,
        anchor: DateTime<Utc>,
        board: BoardView,
    ) -> TimelineMove {
        self.cancel_pending();
        match focus.family() {
            Some(0) => TimelineMove::ToGroupLane,
            Some(family) if family < board.families => TimelineMove::Row {
                family: family - 1,
                anchor,
            },
            None => board
                .families
                .checked_sub(1)
                .map_or(TimelineMove::ToGroupLane, |last| {
                    TimelineMove::Focus(TimelineFocus::Channel(last))
                }),
            _ => TimelineMove::Stay,
        }
    }

    /// Rechecks a deferred Down when a continuation finishes.
    pub fn settle(&mut self, focus: Option<TimelineFocus>, board: BoardView) -> TimelineMove {
        movement(self.down.settle(focus, board))
    }

    /// Keeps a deferred move attached to the same Programme after the row is rebuilt.
    pub(crate) fn rebase_programme(
        &mut self,
        prior: TimelineFocus,
        current: Option<TimelineFocus>,
    ) {
        self.down.rebase_origin(
            prior,
            current.and_then(|focus| focus.family().map(|family| (focus, family))),
        );
    }

    /// Restores board focus after the previously focused control was replaced.
    pub fn recover(&mut self, board: BoardView) -> TimelineMove {
        self.cancel_pending();
        match board.fallback() {
            RowMove::Focus(family, ()) => TimelineMove::Focus(TimelineFocus::Channel(family)),
            RowMove::Retry => TimelineMove::Focus(TimelineFocus::Retry),
            _ => TimelineMove::Stay,
        }
    }

    /// Discards a deferred Down when the user moves horizontally or leaves the board.
    pub fn cancel_pending(&mut self) {
        self.down.clear();
    }
}

fn movement(destination: RowMove<DateTime<Utc>>) -> TimelineMove {
    match destination {
        RowMove::Stay => TimelineMove::Stay,
        RowMove::Focus(family, anchor) => TimelineMove::Row { family, anchor },
        RowMove::Retry => TimelineMove::Focus(TimelineFocus::Retry),
        RowMove::LoadMore => TimelineMove::LoadMore,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};

    use super::*;
    use crate::PageStatus;

    #[test]
    fn retry_returns_to_the_last_channel_or_to_the_lane_when_empty() {
        let now = Utc.with_ymd_and_hms(2026, 11, 24, 9, 0, 0).unwrap();
        let empty = BoardView {
            selection: 1,
            families: 0,
            page: PageStatus::Failed,
            columns: 1,
        };
        let retained = BoardView {
            families: 3,
            ..empty
        };
        let mut navigation = TimelineNavigation::default();
        assert_eq!(
            navigation.recover(empty),
            TimelineMove::Focus(TimelineFocus::Retry)
        );
        assert_eq!(
            navigation.up(TimelineFocus::Retry, now, empty),
            TimelineMove::ToGroupLane
        );
        assert_eq!(
            navigation.up(TimelineFocus::Retry, now, retained),
            TimelineMove::Focus(TimelineFocus::Channel(2)),
        );
        assert_eq!(
            navigation.recover(retained),
            TimelineMove::Focus(TimelineFocus::Channel(0)),
        );
    }

    #[test]
    fn deferred_down_keeps_the_programme_time_until_the_destination_arrives() {
        let now = Utc.with_ymd_and_hms(2026, 11, 24, 9, 0, 0).unwrap();
        let anchor = now + TimeDelta::minutes(75);
        let origin = TimelineFocus::Programme { family: 0, cell: 1 };
        let mut navigation = TimelineNavigation::default();
        let board = BoardView {
            selection: 0,
            families: 1,
            page: PageStatus::Ready,
            columns: 1,
        };
        assert_eq!(
            navigation.down(origin, anchor, board),
            TimelineMove::LoadMore
        );
        assert_eq!(
            navigation.settle(
                Some(origin),
                BoardView {
                    page: PageStatus::Loading,
                    ..board
                }
            ),
            TimelineMove::Stay,
        );
        assert_eq!(
            navigation.settle(
                Some(origin),
                BoardView {
                    families: 2,
                    page: PageStatus::Complete,
                    ..board
                }
            ),
            TimelineMove::Row { family: 1, anchor },
        );
    }
}
