use crate::PageStatus;

const MAX_PENDING_DOWN_STEPS: usize = 8;

/// The rows currently displayed, with a selection identity and their continuation state.
#[derive(Clone, Copy)]
pub struct BoardView {
    pub(crate) selection: u64,
    pub(crate) families: usize,
    pub(crate) page: PageStatus,
}

impl BoardView {
    pub(crate) fn failed(self) -> bool {
        matches!(
            self.page,
            PageStatus::Failed | PageStatus::ReconcileGeneration
        )
    }

    pub(crate) fn fallback(self) -> RowMove<()> {
        if self.families > 0 {
            RowMove::Focus(0, ())
        } else if self.failed() {
            RowMove::Retry
        } else {
            RowMove::Stay
        }
    }
}

pub(crate) enum RowMove<A> {
    Stay,
    Focus(usize, A),
    Retry,
    LoadMore,
}

pub(crate) struct DownSteps<const COLUMNS: usize, F, A> {
    pending: Option<Pending<F, A>>,
}

struct Pending<F, A> {
    origin: F,
    family: usize,
    selection: u64,
    steps: usize,
    anchor: A,
}

impl<const COLUMNS: usize, F, A> Default for DownSteps<COLUMNS, F, A> {
    fn default() -> Self {
        Self { pending: None }
    }
}

impl<const COLUMNS: usize, F: Copy + Eq, A> DownSteps<COLUMNS, F, A> {
    pub(crate) fn down(
        &mut self,
        origin: F,
        family: usize,
        anchor: A,
        board: BoardView,
    ) -> RowMove<A> {
        if family >= board.families {
            self.clear();
            return RowMove::Stay;
        }
        let pending = match self.take_for(Some(origin), board) {
            Some(_) if board.failed() => return RowMove::Retry,
            Some(mut pending) => {
                pending.steps = (pending.steps + 1).min(MAX_PENDING_DOWN_STEPS);
                pending
            }
            None => Pending {
                origin,
                family,
                anchor,
                selection: board.selection,
                steps: 1,
            },
        };
        self.resolve(pending, board)
    }

    pub(crate) fn settle(&mut self, focus: Option<F>, board: BoardView) -> RowMove<A> {
        let Some(pending) = self.take_for(focus, board) else {
            return RowMove::Stay;
        };
        if board.failed() {
            return RowMove::Retry;
        }
        self.resolve(pending, board)
    }

    pub(crate) fn rebase_origin(&mut self, prior: F, current: Option<(F, usize)>) {
        let pending = self
            .pending
            .take()
            .filter(|pending| pending.origin == prior);
        if let (Some(mut pending), Some((origin, family))) = (pending, current) {
            pending.origin = origin;
            pending.family = family;
            self.pending = Some(pending);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.pending = None;
    }

    fn take_for(&mut self, focus: Option<F>, board: BoardView) -> Option<Pending<F, A>> {
        self.pending.take().filter(|pending| {
            focus == Some(pending.origin)
                && board.selection == pending.selection
                && pending.family < board.families
        })
    }

    fn resolve(&mut self, pending: Pending<F, A>, board: BoardView) -> RowMove<A> {
        let target = pending.family.saturating_add(pending.steps * COLUMNS);
        if target < board.families {
            return RowMove::Focus(target, pending.anchor);
        }
        match board.page {
            PageStatus::Ready | PageStatus::Loading => {
                self.pending = Some(pending);
                match board.page {
                    PageStatus::Ready => RowMove::LoadMore,
                    _ => RowMove::Stay,
                }
            }
            PageStatus::Complete | PageStatus::Failed | PageStatus::ReconcileGeneration => {
                let last = board.families - 1;
                let last_line = last / COLUMNS;
                if last_line > pending.family / COLUMNS {
                    let target = (last_line * COLUMNS + pending.family % COLUMNS).min(last);
                    RowMove::Focus(target, pending.anchor)
                } else if board.failed() {
                    RowMove::Retry
                } else {
                    RowMove::Stay
                }
            }
        }
    }
}
