//! Deterministic tab ordering, search and bulk-close planning.
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabMarker {
    #[default]
    None,
    Work,
    Production,
    Development,
    Critical,
}

impl TabMarker {
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::Work,
        Self::Production,
        Self::Development,
        Self::Critical,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Work => "Work",
            Self::Production => "Prod",
            Self::Development => "Dev",
            Self::Critical => "Critical",
        }
    }

    pub fn prefix(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Work => "[W] ",
            Self::Production => "[P] ",
            Self::Development => "[D] ",
            Self::Critical => "[!] ",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseScope {
    Selected,
    RightOf(u64),
    Others(u64),
}

pub fn search_tab_ids<'a>(
    tabs: impl IntoIterator<Item = (u64, &'a str)>,
    query: &str,
) -> Vec<u64> {
    let query = query.trim().to_lowercase();
    tabs.into_iter()
        .filter(|(_, name)| query.is_empty() || name.to_lowercase().contains(&query))
        .map(|(id, _)| id)
        .collect()
}

pub fn move_search_selection(len: usize, current: usize, direction: isize) -> usize {
    if len == 0 || direction == 0 {
        return 0;
    }
    if direction.is_negative() {
        (current + len - 1) % len
    } else {
        (current + 1) % len
    }
}

pub fn close_plan(
    order: &[u64],
    selected: &BTreeSet<u64>,
    scope: CloseScope,
) -> Vec<u64> {
    match scope {
        CloseScope::Selected => order
            .iter()
            .copied()
            .filter(|id| selected.contains(id))
            .collect(),
        CloseScope::RightOf(anchor) => {
            let Some(index) = order.iter().position(|id| *id == anchor) else {
                return Vec::new();
            };
            order[index + 1..].to_vec()
        }
        CloseScope::Others(anchor) => order
            .iter()
            .copied()
            .filter(|id| *id != anchor)
            .collect(),
    }
}

pub fn next_active_after_close(
    order: &[u64],
    active: Option<u64>,
    closing: &BTreeSet<u64>,
) -> Option<u64> {
    let active = active?;
    if !closing.contains(&active) {
        return Some(active);
    }
    let index = order.iter().position(|id| *id == active)?;
    order[index + 1..]
        .iter()
        .chain(order[..index].iter().rev())
        .copied()
        .find(|id| !closing.contains(id))
}

pub fn move_item_by_id<T>(
    items: &mut [T],
    id: u64,
    direction: isize,
    id_of: impl Fn(&T) -> u64,
) -> bool {
    if direction == 0 {
        return false;
    }
    let Some(index) = items.iter().position(|item| id_of(item) == id) else {
        return false;
    };
    let target = if direction.is_negative() {
        index.checked_sub(1)
    } else if index + 1 < items.len() {
        Some(index + 1)
    } else {
        None
    };
    let Some(target) = target else {
        return false;
    };
    items.swap(index, target);
    true
}

pub fn close_items_by_id<T>(
    items: &mut Vec<T>,
    closing: &BTreeSet<u64>,
    id_of: impl Fn(&T) -> u64,
) -> usize {
    let before = items.len();
    items.retain(|item| !closing.contains(&id_of(item)));
    before - items.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        rc::Rc,
    };

    #[test]
    fn search_preserves_tab_order_and_is_case_insensitive() {
        let tabs = [(30, "Prod router"), (10, "Dev shell"), (20, "prod db")];
        assert_eq!(search_tab_ids(tabs, "PROD"), vec![30, 20]);
        assert_eq!(search_tab_ids(tabs, ""), vec![30, 10, 20]);
    }

    #[test]
    fn quick_switcher_keyboard_navigation_wraps_deterministically() {
        assert_eq!(move_search_selection(0, 0, 1), 0);
        assert_eq!(move_search_selection(3, 0, 1), 1);
        assert_eq!(move_search_selection(3, 2, 1), 0);
        assert_eq!(move_search_selection(3, 0, -1), 2);
        assert_eq!(move_search_selection(3, 1, -1), 0);
    }

    #[test]
    fn close_plans_are_deterministic() {
        let order = [10, 20, 30, 40];
        let selected = BTreeSet::from([10, 30]);
        assert_eq!(close_plan(&order, &selected, CloseScope::Selected), vec![10, 30]);
        assert_eq!(close_plan(&order, &selected, CloseScope::RightOf(20)), vec![30, 40]);
        assert_eq!(close_plan(&order, &selected, CloseScope::Others(30)), vec![10, 20, 40]);
    }

    #[test]
    fn active_falls_forward_then_left_when_closed() {
        let order = [10, 20, 30, 40];
        assert_eq!(
            next_active_after_close(&order, Some(20), &BTreeSet::from([20, 30])),
            Some(40)
        );
        assert_eq!(
            next_active_after_close(&order, Some(40), &BTreeSet::from([30, 40])),
            Some(20)
        );
        assert_eq!(
            next_active_after_close(&order, Some(20), &BTreeSet::from([10, 20, 30, 40])),
            None
        );
    }

    #[test]
    fn move_changes_only_adjacent_order() {
        let mut items = vec![(10, "a"), (20, "b"), (30, "c")];
        assert!(move_item_by_id(&mut items, 20, -1, |item| item.0));
        assert_eq!(items.iter().map(|item| item.0).collect::<Vec<_>>(), vec![20, 10, 30]);
        assert!(!move_item_by_id(&mut items, 20, -1, |item| item.0));
        assert!(move_item_by_id(&mut items, 20, 1, |item| item.0));
        assert_eq!(items.iter().map(|item| item.0).collect::<Vec<_>>(), vec![10, 20, 30]);
    }

    struct DropSpy {
        id: u64,
        drops: Rc<Cell<usize>>,
    }

    impl Drop for DropSpy {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    #[test]
    fn bulk_close_drops_each_removed_item_exactly_once() {
        let drops = Rc::new(Cell::new(0));
        let mut items = vec![
            DropSpy { id: 10, drops: drops.clone() },
            DropSpy { id: 20, drops: drops.clone() },
            DropSpy { id: 30, drops: drops.clone() },
        ];
        let removed = close_items_by_id(&mut items, &BTreeSet::from([10, 30]), |item| item.id);
        assert_eq!(removed, 2);
        assert_eq!(drops.get(), 2);
        assert_eq!(items.len(), 1);
        drop(items);
        assert_eq!(drops.get(), 3);
    }
}
