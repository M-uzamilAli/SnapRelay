//! Context Buffer: the one place screenshots live, in memory only.
//! Everything in it is waiting to be delivered; a screenshot is removed (and its
//! memory freed) as soon as the target app has taken it, or when the user removes it.

use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::capture::Frame;

pub struct Shot {
    pub id: u32,
    pub frame: Frame,
    /// Local capture time, "20:41".
    pub time: String,
}

#[derive(Default)]
pub struct Buffer {
    items: Vec<Shot>,
    next_id: u32,
    /// Goes up on every change, so the queue window knows when to redraw.
    version: u64,
}

impl Buffer {
    pub fn add(&mut self, frame: Frame) -> u32 {
        self.next_id += 1;
        let id = self.next_id;
        let now = unsafe { GetLocalTime() };
        let time = format!("{:02}:{:02}", now.wHour, now.wMinute);
        self.items.push(Shot { id, frame, time });
        self.version += 1;
        id
    }

    /// Everything queued, in capture order.
    pub fn items(&self) -> &[Shot] {
        &self.items
    }

    /// Ids in capture order.
    pub fn pending_ids(&self) -> Vec<u32> {
        self.items.iter().map(|s| s.id).collect()
    }

    pub fn get(&self, id: u32) -> Option<&Shot> {
        self.items.iter().find(|s| s.id == id)
    }

    /// Delivered or removed: drop it, which frees its memory.
    pub fn remove(&mut self, id: u32) {
        self.items.retain(|s| s.id != id);
        self.version += 1;
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.version += 1;
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// (queued count, total compressed bytes in memory)
    pub fn summary(&self) -> (usize, usize) {
        (self.items.len(), self.items.iter().map(|s| s.frame.png.len()).sum())
    }
}
