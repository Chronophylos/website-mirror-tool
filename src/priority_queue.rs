use std::sync::Arc;

use crossbeam_queue::SegQueue;
use dashmap::DashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    Normal,
    Low,
}

impl Priority {
    fn len() -> usize {
        2
    }
}

impl Default for Priority {
    fn default() -> Self {
        Self::Normal
    }
}

/// A priority queue
#[derive(Debug, Clone)]
pub struct PriorityQueue<T> {
    queues: DashMap<Priority, Arc<SegQueue<T>>>,
}

impl<T> PriorityQueue<T> {
    pub fn new() -> Self {
        let queues = DashMap::with_capacity(Priority::len());
        queues.insert(Priority::Normal, Arc::new(SegQueue::new()));
        queues.insert(Priority::Low, Arc::new(SegQueue::new()));

        Self { queues }
    }

    fn pop_priority(&self, priority: Priority) -> Option<T> {
        self.queues
            .get(&priority)
            .map(|queue| queue.pop())
            .flatten()
    }

    pub fn pop(&self) -> Option<T> {
        self.pop_priority(Priority::Normal)
            .or_else(|| self.pop_priority(Priority::Low))
    }

    pub fn push<P>(&self, value: T, priority: P)
    where
        P: Into<Option<Priority>>,
    {
        self.queues
            .get(&priority.into().unwrap_or_default())
            .map(|queue| queue.push(value));
    }

    pub fn is_empty(&self) -> bool {
        self.queues.iter().all(|queue| queue.is_empty())
    }
}
