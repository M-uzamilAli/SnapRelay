//! Context Buffer: the one place screenshots live, in memory only.

use crate::capture::Frame;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Pending,
    /// Ctrl+V was sent. Not yet verified that the app attached it.
    Pasted,
}

pub struct Shot {
    pub id: u32,
    pub frame: Frame,
    pub status: Status,
}

#[derive(Default)]
pub struct Buffer {
    items: Vec<Shot>,
    next_id: u32,
}

impl Buffer {
    pub fn add(&mut self, frame: Frame) -> u32 {
        self.next_id += 1;
        let id = self.next_id;
        self.items.push(Shot { id, frame, status: Status::Pending });
        id
    }

    pub fn pending_ids(&self) -> Vec<u32> {
        self.items
            .iter()
            .filter(|s| s.status == Status::Pending)
            .map(|s| s.id)
            .collect()
    }

    pub fn get(&self, id: u32) -> Option<&Shot> {
        self.items.iter().find(|s| s.id == id)
    }

    pub fn mark(&mut self, id: u32, status: Status) {
        if let Some(s) = self.items.iter_mut().find(|s| s.id == id) {
            s.status = status;
        }
    }

    /// (pending count, total count, total compressed bytes)
    pub fn summary(&self) -> (usize, usize, usize) {
        let pending = self.items.iter().filter(|s| s.status == Status::Pending).count();
        let bytes = self.items.iter().map(|s| s.frame.png.len()).sum();
        (pending, self.items.len(), bytes)
    }
}
