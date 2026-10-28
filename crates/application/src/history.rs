use std::collections::BTreeSet;

/// An empty item selection is a no-op, distinct from clearing all history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryTarget {
    All,
    Items(BTreeSet<String>),
}
impl HistoryTarget {
    pub fn single(id: impl Into<String>) -> Self {
        Self::Items([id.into()].into())
    }
    pub fn contains(&self, id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Items(ids) => ids.contains(id),
        }
    }
}
