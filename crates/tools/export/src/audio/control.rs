//! 音频导出控制 — 暂停/继续与中止
//!
//! 为 CPU (xsynth) 与 GPU (lumino-gpu-synth) 双后端提供统一的协作式控制。
//! 渲染线程在关键循环点调用 `check_abort` / `wait_if_paused`，UI 线程通过
//! `AudioExportControl` 的原子标志驱动暂停与中止。

use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};

use crate::error::{ExportError, ExportResult};

/// 音频导出控制句柄（跨线程共享）
///
/// `paused` 与 `aborted` 为原子标志，`pause_mutex`/`pause_cond` 用于
/// 暂停时的阻塞等待，避免忙循环空转。
#[derive(Debug, Default)]
pub struct AudioExportControl {
    paused: AtomicBool,
    aborted: AtomicBool,
    pause_mutex: Mutex<bool>,
    pause_cond: Condvar,
}

impl AudioExportControl {
    /// 创建新的控制句柄（初始为运行状态）
    pub fn new() -> Self {
        Self {
            paused: AtomicBool::new(false),
            aborted: AtomicBool::new(false),
            pause_mutex: Mutex::new(false),
            pause_cond: Condvar::new(),
        }
    }

    /// 请求暂停（幂等）
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
        *self.pause_mutex.lock().expect("pause mutex poisoned") = true;
    }

    /// 请求继续（幂等）
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
        *self.pause_mutex.lock().expect("pause mutex poisoned") = false;
        self.pause_cond.notify_all();
    }

    /// 切换暂停状态，返回切换后的是否暂停
    pub fn toggle_pause(&self) -> bool {
        if self.is_paused() {
            self.resume();
            false
        } else {
            self.pause();
            true
        }
    }

    /// 请求中止（幂等，会自动唤醒暂停中的线程）
    pub fn abort(&self) {
        self.aborted.store(true, Ordering::Relaxed);
        // 唤醒可能在 pause_cond 上等待的线程
        self.resume();
    }

    /// 是否处于暂停状态
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// 是否已请求中止
    pub fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::Relaxed)
    }

    /// 若已中止则返回 `ExportError::Aborted`
    pub fn check_abort(&self) -> ExportResult<()> {
        if self.is_aborted() {
            return Err(ExportError::Aborted);
        }
        Ok(())
    }

    /// 若处于暂停则阻塞直到恢复或中止
    ///
    /// 使用 `Condvar` 等待，避免忙循环。每次等待最多 100ms 后
    /// 重新检查 `aborted`，确保中止能及时响应。
    pub fn wait_if_paused(&self) {
        while self.is_paused() && !self.is_aborted() {
            let guard = self.pause_mutex.lock().expect("pause mutex poisoned");
            if !*guard {
                break;
            }
            let (guard, _) = self
                .pause_cond
                .wait_timeout(guard, std::time::Duration::from_millis(100))
                .expect("condvar wait failed");
            drop(guard);
            if !self.is_paused() || self.is_aborted() {
                break;
            }
        }
    }

    /// 组合检查：先处理暂停等待，再检查中止
    pub fn checkpoint(&self) -> ExportResult<()> {
        self.wait_if_paused();
        self.check_abort()
    }
}

/// 供 `AudioRenderConfig` 使用的共享控制别名
pub type SharedControl = Arc<AudioExportControl>;
