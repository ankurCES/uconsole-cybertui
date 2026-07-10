use crate::app::screen::ScreenId;

pub struct MenuStack {
    frames: Vec<ScreenId>, // invariant: len >= 1, frames[0] == Overworld
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopResult {
    Ok(ScreenId),  // new top after pop
    WouldExit,     // only Overworld remains — caller opens quit modal
}

impl Default for MenuStack {
    fn default() -> Self {
        Self::new()
    }
}

impl MenuStack {
    pub fn new() -> Self {
        Self { frames: vec![ScreenId::Overworld] }
    }

    /// Construct a stack with a custom root (e.g. MainMenu for the v2 event loop).
    /// Does NOT affect `new()` so existing tests stay green.
    pub fn with_root(id: ScreenId) -> Self {
        Self { frames: vec![id] }
    }

    pub fn current(&self) -> ScreenId {
        *self.frames.last().unwrap()
    }

    /// Push a new screen. Idempotent if already at the top.
    pub fn push(&mut self, id: ScreenId) {
        if self.frames.last() != Some(&id) {
            self.frames.push(id);
        }
    }

    pub fn pop(&mut self) -> PopResult {
        if self.frames.len() <= 1 {
            PopResult::WouldExit
        } else {
            self.frames.pop();
            PopResult::Ok(self.current())
        }
    }

    /// Iterator from Overworld (bottom) to current screen (top).
    pub fn breadcrumb(&self) -> impl Iterator<Item = ScreenId> + '_ {
        self.frames.iter().copied()
    }

    pub fn depth(&self) -> usize {
        self.frames.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_overworld_depth_one() {
        let s = MenuStack::new();
        assert_eq!(s.current(), ScreenId::Overworld);
        assert_eq!(s.depth(), 1);
    }

    #[test]
    fn push_increases_depth_and_updates_current() {
        let mut s = MenuStack::new();
        s.push(ScreenId::Network);
        assert_eq!(s.current(), ScreenId::Network);
        assert_eq!(s.depth(), 2);
    }

    #[test]
    fn push_same_id_is_idempotent() {
        let mut s = MenuStack::new();
        s.push(ScreenId::Network);
        s.push(ScreenId::Network);
        assert_eq!(s.depth(), 2);
    }

    #[test]
    fn push_three_deep() {
        let mut s = MenuStack::new();
        s.push(ScreenId::Files);
        s.push(ScreenId::Editor);
        assert_eq!(s.depth(), 3);
        assert_eq!(s.current(), ScreenId::Editor);
    }

    #[test]
    fn pop_at_root_returns_would_exit_and_leaves_overworld() {
        let mut s = MenuStack::new();
        assert_eq!(s.pop(), PopResult::WouldExit);
        assert_eq!(s.current(), ScreenId::Overworld);
    }

    #[test]
    fn pop_returns_new_top() {
        let mut s = MenuStack::new();
        s.push(ScreenId::Network);
        s.push(ScreenId::System);
        assert_eq!(s.pop(), PopResult::Ok(ScreenId::Network));
        assert_eq!(s.current(), ScreenId::Network);
    }

    #[test]
    fn breadcrumb_yields_full_path() {
        let mut s = MenuStack::new();
        s.push(ScreenId::Network);
        s.push(ScreenId::System);
        let path: Vec<_> = s.breadcrumb().collect();
        assert_eq!(
            path,
            vec![ScreenId::Overworld, ScreenId::Network, ScreenId::System]
        );
    }
}
