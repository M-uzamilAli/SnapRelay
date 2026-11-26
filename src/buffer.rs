//! Context Buffer: the one place screenshots live, in memory only.
//! Everything in it is waiting to be delivered; a screenshot is removed (and its
//! memory freed) once the target app has taken it, or when the user removes it.
//! Its order is the sending order, and each screenshot carries its own tick
//! (send it or not) and delivery status, so the queue window and the release
//! shortcut always agree.

use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::capture::Frame;

/// Where a screenshot is in the current send.
#[derive(Clone, Copy, PartialEq)]
pub enum Status {
    /// Not part of a send right now.
    Waiting,
    /// Being pasted: (this one, of how many).
    Sending(usize, usize),
    /// The app took it; removed when the send finishes.
    Sent,
    /// The send stopped at this one; it stays queued.
    NotSent,
}

pub struct Shot {
    pub id: u32,
    pub frame: Frame,
    /// Local capture time, "20:41".
    pub time: String,
    /// Ticked = goes out with the next send. New screenshots start ticked.
    pub selected: bool,
    pub status: Status,
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
        self.items.push(Shot { id, frame, time, selected: true, status: Status::Waiting });
        self.version += 1;
        id
    }

    /// Everything queued, in sending order.
    pub fn items(&self) -> &[Shot] {
        &self.items
    }

    /// Ticked ids, in sending order.
    pub fn selected_ids(&self) -> Vec<u32> {
        self.items.iter().filter(|s| s.selected).map(|s| s.id).collect()
    }

    pub fn get(&self, id: u32) -> Option<&Shot> {
        self.items.iter().find(|s| s.id == id)
    }

    fn get_mut(&mut self, id: u32) -> Option<&mut Shot> {
        self.items.iter_mut().find(|s| s.id == id)
    }

    pub fn toggle(&mut self, id: u32) {
        if let Some(s) = self.get_mut(id) {
            s.selected = !s.selected;
            s.status = Status::Waiting;
            self.version += 1;
        }
    }

    /// Tick everything, or untick everything if all are already ticked.
    pub fn toggle_all(&mut self) {
        let tick = !self.items.iter().all(|s| s.selected);
        for s in &mut self.items {
            s.selected = tick;
        }
        self.version += 1;
    }

    /// Move a screenshot to position `to` (0 = first to be sent).
    pub fn move_to(&mut self, id: u32, to: usize) {
        if let Some(from) = self.items.iter().position(|s| s.id == id) {
            let shot = self.items.remove(from);
            let to = to.min(self.items.len());
            self.items.insert(to, shot);
            self.version += 1;
        }
    }

    pub fn set_status(&mut self, id: u32, status: Status) {
        if let Some(s) = self.get_mut(id) {
            s.status = status;
            self.version += 1;
        }
    }

    /// A new send is starting: forget the previous one's results.
    pub fn reset_status(&mut self) {
        for s in &mut self.items {
            s.status = Status::Waiting;
        }
        self.version += 1;
    }

    /// Drop everything the app took, which frees its memory. Returns how many.
    pub fn remove_sent(&mut self) -> usize {
        let before = self.items.len();
        self.items.retain(|s| s.status != Status::Sent);
        self.version += 1;
        before - self.items.len()
    }

    /// Removed by the user: drop it, which frees its memory.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame { width: 1, height: 1, png: vec![0; 10], preview: (1, 1, vec![0; 4]) }
    }

    #[test]
    fn ticks_order_and_send_results() {
        let mut b = Buffer::default();
        let (a, c, d) = (b.add(frame()), b.add(frame()), b.add(frame()));
        assert_eq!(b.selected_ids(), vec![a, c, d]); // new ones start ticked
        b.toggle(c);
        assert_eq!(b.selected_ids(), vec![a, d]); // unticked ones are skipped
        b.move_to(d, 0);
        assert_eq!(b.selected_ids(), vec![d, a]); // order = sending order
        b.set_status(d, Status::Sent);
        b.set_status(a, Status::NotSent);
        assert_eq!(b.remove_sent(), 1);
        let left: Vec<u32> = b.items().iter().map(|s| s.id).collect();
        assert_eq!(left, vec![a, c]); // failed and unticked stay
        b.toggle_all();
        assert_eq!(b.selected_ids(), vec![a, c]);
        b.toggle_all();
        assert!(b.selected_ids().is_empty());
    }
}
