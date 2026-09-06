//! 对话框事件处理模块

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use lumino_ui::message::SaveConfirmAction;
use lumino_ui::state::root_state::DialogType;

use crate::runner::dialog_manager::DialogResult;
use crate::runner::inner::RunnerInner;

impl RunnerInner {
    /// 处理对话框窗口事件
    pub(crate) fn handle_dialog_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) -> bool {
        // 检查是否是对话框窗口
        if !self.window_state.dialog_manager.is_dialog_window(window_id) {
            return false;
        }

        let mut dialog_result = None;
        let mut should_close = false;
        // 记录对话框类型（用于块外判断保存确认对话框被 X 关闭的情况）
        let mut dialog_type: Option<DialogType> = None;

        if let Some(dialog) = self.window_state.dialog_manager.get_dialog_mut(window_id) {
            dialog.handle_event(event);
            dialog_type = Some(dialog.dialog_type());

            // 检查对话框是否应该关闭
            should_close = dialog.should_close();
            tracing::debug!("对话框 should_close: {}", should_close);

            // 检查对话框结果（handle_event 只入队事件，不会处理消息）
            if let Some(result) = dialog.check_result() {
                tracing::debug!(
                    "对话框结果（第一次检查）: {:?}",
                    std::mem::discriminant(&result)
                );
                // 永久删除音轨缓存：对话框保持开启（Runner 处理完后刷新条目列表），
                // 其余结果沿用"取到结果即关闭"的默认行为。
                should_close =
                    !matches!(result, DialogResult::RecoverTrackPermanentlyDelete { .. });
                dialog_result = Some(result);
            }

            // 请求重绘并处理待处理事件（process_pending_events）
            // 注意：事件处理在 redraw 中同步执行，可能产生 dialog_result
            if !should_close {
                tracing::debug!("调用 dialog.redraw()");
                dialog.redraw();
                // 应用自制标题栏产生的窗口动作（关闭 / 最小化 / 拖动）
                // 必须在 redraw（事件队列已消费）之后调用
                dialog.apply_window_actions();
            } else {
                tracing::debug!("跳过 dialog.redraw()，因为 should_close = true");
            }

            // 再次检查结果：redraw 可能处理了事件并设置了 dialog_result
            if dialog_result.is_none() {
                should_close = dialog.should_close();
                if let Some(result) = dialog.check_result() {
                    tracing::debug!(
                        "对话框结果（第二次检查）: {:?}",
                        std::mem::discriminant(&result)
                    );
                    // 永久删除音轨缓存：对话框保持开启（Runner 处理完后刷新条目列表），
                    // 其余结果沿用"取到结果即关闭"的默认行为。
                    should_close =
                        !matches!(result, DialogResult::RecoverTrackPermanentlyDelete { .. });
                    dialog_result = Some(result);
                }
            }
        }

        // 如果应该关闭，关闭对话框
        if should_close {
            self.window_state.dialog_manager.close_dialog(window_id);
        }

        // 保存确认对话框被窗口 X 关闭（未点击任何按钮）→ 视为「取消」关闭操作，
        // 清空挂起的关闭动作，避免后续关闭请求被忽略。
        if dialog_result.is_none() && should_close && dialog_type == Some(DialogType::SaveConfirm) {
            tracing::debug!("保存确认对话框被 X 关闭，视为取消");
            self.handle_save_confirm_result(SaveConfirmAction::Cancel);
        }

        // 处理对话框返回的结果
        if let Some(result) = dialog_result {
            tracing::debug!("处理对话框结果: {:?}", std::mem::discriminant(&result));
            self.process_dialog_result(result);
        }

        true
    }

    /// 处理对话框结果
    fn process_dialog_result(&mut self, result: DialogResult) {
        match &result {
            DialogResult::SaveConfirm(choice) => {
                // 保存确认对话框：保存 / 关闭（放弃）/ 取消 三选一
                // 交由 RunnerInner 继续挂起的关闭动作或清空挂起状态。
                self.handle_save_confirm_result(*choice);
            }
            DialogResult::LoadConfirm => {
                if let Some(path) = self.file_state.pending_load_path.take() {
                    self.load_midi_file(path);
                } else {
                    tracing::warn!("LoadConfirm: 没有 pending 的加载路径");
                }
            }
            DialogResult::Cancel => {
                tracing::info!("对话框: 取消");
            }
            DialogResult::ProjectSettings {
                title,
                tempo,
                copyright,
                author,
                time_signatures,
            } => {
                tracing::info!(
                    "应用工程设置(对话框结果): 标题={}, BPM={}, 版权={}, 作者={}, 拍号变化数={}",
                    title,
                    tempo,
                    copyright,
                    author,
                    time_signatures.len()
                );
                let main_ui = self.window_state.window.ui_mut();
                main_ui.apply_project_settings(
                    title.clone(),
                    *tempo,
                    copyright.clone(),
                    author.clone(),
                    time_signatures.clone(),
                );
                // 更新主窗口标题
                self.window_state
                    .window
                    .window()
                    .set_title(&format!("{} - Lumino", title));
            }
            DialogResult::Settings { settings, theme } => {
                tracing::info!("应用设置面板配置到主窗口，主题: {}", theme);
                let main_ui = self.window_state.window.ui_mut();
                main_ui.apply_settings((**settings).clone(), theme.clone());
            }
            _ => {
                let is_permanent_delete =
                    matches!(&result, DialogResult::RecoverTrackPermanentlyDelete { .. });
                let main_ui = self.window_state.window.ui_mut();
                crate::runner::inner::RunnerInner::apply_dialog_result_to_ui(main_ui, result);
                // 永久删除音轨缓存后对话框保持开启：重新扫描缓存目录，下一帧
                // try_fill_recover_track_entries 会把新列表注入对话框，刷新已删除条目。
                if is_permanent_delete {
                    let entries = self.scan_recover_track_entries();
                    tracing::info!(
                        "找回删除音轨：永久删除后重新扫描，剩余缓存 {} 个",
                        entries.len()
                    );
                    self.pending_recover_track_entries = Some(entries);
                }
            }
        }
    }
}
