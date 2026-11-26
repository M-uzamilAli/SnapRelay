//! Context Buffer: the one place screenshots live, in memory only.
//! Everything in it is waiting to be delivered; a screenshot is removed (and its
//! memory freed) as soon as the target app has taken it.

use crate::capture::Frame;

pub struct Shot {
    pub id: u32,
    pub frame: Frame,
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
        self.items.push(Shot { id, frame });
        id
    }

    /// Ids in capture order.
    pub fn pending_ids(&self) -> Vec<u32> {
        self.items.iter().map(|s| s.id).collect()
    }

    pub fn get(&self, id: u32) -> Option<&Shot> {
        self.items.iter().find(|s| s.id == id)
    }

    /// Delivered: drop it, which frees its memory.
    pub fn remove(&mut self, id: u32) {
        self.items.retain(|s| s.id != id);
    }

    /// (queued count, total compressed bytes in memory)
    pub fn summary(&self) -> (usize, usize) {
        (self.items.len(), self.items.iter().map(|s| s.frame.png.len()).sum())
    }
}
