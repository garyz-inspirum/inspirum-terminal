//! Deterministic tab ordering, search, labels and bulk-close planning.
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabVisualLabel {
    #[default]
    None,
    Blue,
    Green,
    Amber,
    Red,
}

impl TabVisualLabel {
    pub const ALL: [Self; 5] = [Self::None, Self::Blue, Self::Green, Self::Amber, Self::Red];

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Blue => "Blue",
            Self::Green => "Green",
            Self::Amber => "Amber",
            Self::Red => "Red",
        }
    }

    pub fn marker(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Blue => "● ",
            Self::Green => "◆ ",
            Self::Amber => "▲ ",
            Self::Red => "■ ",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BulkCloseMode {
    Selected,
    RightOf(u64),
    Others(u64),
}

pub fn matching_tab_ids<'a>(
    tabs: impl IntoIterator<Item = (u64, &'a str)>,
    query: &str,
) -> Vec<u64> {
    let query = query.trim().to_lowercase();
    tabs.into_iter()
        .filter_map(|(id, name)| {
            (query.is_empty() || name.to_lowercase().contains(&query)).then_some(id)
        })
        .collect()
}

pub fn moved_order(order: &[u64], id: u64, delta: isize) -> Vec<u64> {
    let mut next = order.to_vec();
    let Some(index) = next.iter().position(|candidate| *candidate == id) else {
        return next;
    };
    let target = if delta < 0 {
        index.saturating_sub(delta.unsigned_abs())
    } else {
        index
            .saturating_add(delta as usize)
            .min(next.len().saturating_sub(1))
    };
    if index != target {
        let id = next.remove(index);
        next.insert(target, id);
    }
    next
}

pub fn bulk_close_ids(order: &[u64], selected: &BTreeSet<u64>, mode: BulkCloseMode) -> Vec<u64> {
    match mode {
        BulkCloseMode::Selected => order
            .iter()
            .copied()
            .filter(|id| selected.contains(id))
            .collect(),
        BulkCloseMode::RightOf(anchor) => {
            let Some(index) = order.iter().position(|id| *id == anchor) else {
                return Vec::new();
            };
            order[index + 1..].to_vec()
        }
        BulkCloseMode::Others(anchor) => order.iter().copied().filter(|id| *id != anchor).collect(),
    }
}

pub fn next_active_after_close(order: &[u64], active: Option<u64>, closing: &[u64]) -> Option<u64> {
    let closing: BTreeSet<u64> = closing.iter().copied().collect();
    if let Some(active) = active
        && !closing.contains(&active)
        && order.contains(&active)
    {
        return Some(active);
    }

    let active_index = active.and_then(|id| order.iter().position(|candidate| *candidate == id));
    if let Some(index) = active_index {
        for id in order[index + 1..].iter().chain(order[..index].iter().rev()) {
            if !closing.contains(id) {
                return Some(*id);
            }
        }
    }
    order.iter().copied().find(|id| !closing.contains(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_search_is_case_insensitive_and_preserves_order() {
        let tabs = [(10, "Prod Router"), (20, "db-shell"), (30, "PROD API")];
        assert_eq!(matching_tab_ids(tabs, "prod"), vec![10, 30]);
        assert_eq!(matching_tab_ids(tabs, ""), vec![10, 20, 30]);
    }

    #[test]
    fn move_left_and_right_are_deterministic() {
        let order = [1, 2, 3, 4];
        assert_eq!(moved_order(&order, 3, -1), vec![1, 3, 2, 4]);
        assert_eq!(moved_order(&order, 2, 2), vec![1, 3, 4, 2]);
        assert_eq!(moved_order(&order, 1, -1), order);
        assert_eq!(moved_order(&order, 4, 1), order);
    }

    #[test]
    fn bulk_close_modes_keep_source_order() {
        let order = [1, 2, 3, 4];
        let selected = BTreeSet::from([2, 4]);
        assert_eq!(
            bulk_close_ids(&order, &selected, BulkCloseMode::Selected),
            vec![2, 4]
        );
        assert_eq!(
            bulk_close_ids(&order, &selected, BulkCloseMode::RightOf(2)),
            vec![3, 4]
        );
        assert_eq!(
            bulk_close_ids(&order, &selected, BulkCloseMode::Others(3)),
            vec![1, 2, 4]
        );
    }

    #[test]
    fn active_selection_moves_predictably_after_close() {
        let order = [1, 2, 3, 4];
        assert_eq!(next_active_after_close(&order, Some(2), &[2]), Some(3));
        assert_eq!(next_active_after_close(&order, Some(4), &[4]), Some(3));
        assert_eq!(next_active_after_close(&order, Some(3), &[1, 2]), Some(3));
        assert_eq!(
            next_active_after_close(&order, Some(1), &[1, 2, 3, 4]),
            None
        );
    }
}
