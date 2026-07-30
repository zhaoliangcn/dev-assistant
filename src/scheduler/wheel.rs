//! 分层时间轮 (TimingWheel)。
//!
//! 使用单层时间轮，3600 个槽（精度 1 秒，覆盖 1 小时）。
//! 超过 1 小时的任务放入"溢出队列"，定期检查。
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::scheduler::task::{ScheduledTask, ScheduledTaskId};

/// 时间轮槽中的条目
#[derive(Debug, Clone)]
struct SlotEntry {
    task_id: ScheduledTaskId,
    #[allow(dead_code)]
    epoch: u64,
}

/// 时间轮
pub struct TimingWheel {
    /// 槽位（秒级精度，3600 个槽 = 1 小时）
    slots: Mutex<Vec<Vec<SlotEntry>>>,
    /// 当前指针位置（epoch 秒）
    cursor: AtomicU64,
    /// 槽大小（秒）
    slot_size: u64,
    /// 总槽数
    num_slots: u64,
    /// 溢出队列（超过 1 小时的任务）
    overflow: Mutex<HashMap<ScheduledTaskId, SlotEntry>>,
}

impl TimingWheel {
    /// 创建一个新的时间轮。
    ///
    /// `slot_size`: 每个槽的秒数（默认 1）
    /// `num_slots`: 槽数量（默认 3600）
    pub fn new(slot_size: u64, num_slots: u64) -> Self {
        let slots = (0..num_slots).map(|_| Vec::new()).collect();
        Self {
            slots: Mutex::new(slots),
            cursor: AtomicU64::new(0),
            slot_size,
            num_slots,
            overflow: Mutex::new(HashMap::new()),
        }
    }

    /// 创建默认时间轮（1秒精度，3600槽 = 1小时覆盖）。
    pub fn default() -> Self {
        Self::new(1, 3600)
    }

    /// 添加任务到时间轮。
    pub fn add_task(&self, task: &ScheduledTask) {
        let epoch = task.next_run_at as u64;
        let now = chrono::Utc::now().timestamp() as u64;

        let cursor = self.cursor.load(Ordering::Relaxed);
        if cursor == 0 {
            self.cursor.store(now, Ordering::Relaxed);
        }

        let cursor = self.cursor.load(Ordering::Relaxed);

        // 如果 epoch 已经过去或超出时间轮范围，放入溢出队列
        if epoch <= cursor || epoch > cursor + self.num_slots * self.slot_size {
            let mut overflow = self.overflow.lock().unwrap();
            overflow.insert(task.id.clone(), SlotEntry {
                task_id: task.id.clone(),
                epoch,
            });
            return;
        }

        let offset = epoch - cursor;
        let slot_idx = (offset / self.slot_size) % self.num_slots;

        let mut slots = self.slots.lock().unwrap();
        slots[slot_idx as usize].push(SlotEntry {
            task_id: task.id.clone(),
            epoch,
        });
    }

    /// 移除任务。
    pub fn remove_task(&self, task_id: &str) -> bool {
        // 从溢出队列中移除
        {
            let mut overflow = self.overflow.lock().unwrap();
            if overflow.remove(task_id).is_some() {
                return true;
            }
        }

        // 从槽中移除
        let mut slots = self.slots.lock().unwrap();
        for slot in slots.iter_mut() {
            if let Some(pos) = slot.iter().position(|e| e.task_id == task_id) {
                slot.remove(pos);
                return true;
            }
        }
        false
    }

    /// 推进指针，返回当前 tick 到期的任务 ID 列表。
    #[allow(dead_code)]
    pub fn tick(&self) -> Vec<ScheduledTaskId> {
        let now = chrono::Utc::now().timestamp() as u64;
        let cursor = self.cursor.load(Ordering::Relaxed);

        if cursor == 0 {
            self.cursor.store(now, Ordering::Relaxed);
            return Vec::new();
        }

        if now < cursor {
            // 时间未推进
            return Vec::new();
        }

        let mut due_tasks = Vec::new();

        // 处理从 cursor 到 now 之间的所有槽
        let mut current_cursor = cursor;
        while current_cursor <= now {
            let offset = current_cursor - cursor;
            let slot_idx = (offset / self.slot_size) % self.num_slots;

            let mut slots = self.slots.lock().unwrap();
            let slot = &mut slots[slot_idx as usize];

            // 取出该槽中所有到期任务
            let mut i = 0;
            while i < slot.len() {
                if slot[i].epoch <= current_cursor {
                    due_tasks.push(slot.remove(i).task_id);
                } else {
                    i += 1;
                }
            }
            drop(slots);

            current_cursor += self.slot_size;
        }

        // 更新指针到当前时间
        self.cursor.store(now, Ordering::Relaxed);

        // 检查溢出队列：将到期任务取出，将可放入时间轮的重新加入
        let mut overflow = self.overflow.lock().unwrap();
        let mut to_promote = Vec::new();
        overflow.retain(|task_id, entry| {
            if entry.epoch <= now {
                // 到期
                due_tasks.push(task_id.clone());
                false
            } else if entry.epoch <= now + self.num_slots * self.slot_size {
                // 在时间轮范围内，提升到时间轮
                to_promote.push(entry.clone());
                false
            } else {
                // 仍然超出范围，留在溢出队列
                true
            }
        });
        drop(overflow);

        // 将可放入时间轮的任务重新加入
        for entry in to_promote {
            let offset = entry.epoch - now;
            let slot_idx = (offset / self.slot_size) % self.num_slots;
            let mut slots = self.slots.lock().unwrap();
            slots[slot_idx as usize].push(entry);
        }

        due_tasks
    }

    /// 清空时间轮。
    #[allow(dead_code)]
    pub fn clear(&self) {
        let mut slots = self.slots.lock().unwrap();
        for slot in slots.iter_mut() {
            slot.clear();
        }
        let mut overflow = self.overflow.lock().unwrap();
        overflow.clear();
        self.cursor.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::task::*;

    fn create_test_task(id: &str) -> ScheduledTask {
        ScheduledTask::new(
            id.to_string(),
            "test".to_string(),
            ScheduleType::Interval(60),
            TaskExecutionMode::Agent { instruction: "test".to_string() },
            0,
            vec![],
            0,
        )
    }

    #[test]
    fn test_wheel_add_and_remove() {
        let wheel = TimingWheel::default();
        let mut task = create_test_task("test_1");
        task.next_run_at = chrono::Utc::now().timestamp() + 10; // 10秒后
        wheel.add_task(&task);
        assert!(wheel.remove_task("test_1"));
        assert!(!wheel.remove_task("test_1"));
    }

    #[test]
    fn test_wheel_tick_no_due() {
        let wheel = TimingWheel::default();
        let due = wheel.tick();
        assert!(due.is_empty());
    }

    #[test]
    fn test_wheel_overflow() {
        let wheel = TimingWheel::default();
        let mut task = create_test_task("far_future");
        // 2小时后
        task.next_run_at = chrono::Utc::now().timestamp() + 7200;
        wheel.add_task(&task);

        // 应该存在于溢出队列中
        let overflow = wheel.overflow.lock().unwrap();
        assert!(overflow.contains_key("far_future"));
        drop(overflow);

        // tick 不应触发
        let due = wheel.tick();
        assert!(!due.contains(&"far_future".to_string()));
    }
}