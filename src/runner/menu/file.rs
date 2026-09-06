//! Runner 文件菜单处理

mod editor_midi;
mod export;
mod helpers;
mod load;
mod material;
mod save;

use winit::event_loop::ActiveEventLoop;

use lumino_ui::message::SaveConfirmAction;
use lumino_ui::state::root_state::DialogType;

use crate::runner::RunnerInner;
use crate::runner::inner::PendingCloseAction;

impl RunnerInner {
    /// 处理文件菜单事件
    pub(super) fn handle_file_menu_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        file_event: lumino_ui::event::menu::file::Event,
    ) {
        use lumino_ui::event::menu::file::Event::*;

        match file_event {
            Exit => {
                // 退出前检查未保存更改；有则弹保存确认对话框，无则直接退出
                self.request_close_action(PendingCloseAction::Exit, event_loop);
            }
            New => {
                // 新建前检查未保存更改；有则弹保存确认对话框，无则直接新建
                self.request_close_action(PendingCloseAction::NewProject, event_loop);
            }
            Open => {
                // 打开另一个工程前检查未保存更改；有则弹保存确认对话框
                self.request_close_action(PendingCloseAction::OpenProject, event_loop);
            }
            ImportFiles => self.handle_import_files(),
            ImportFromCloud => {
                self.ensure_cloud_ready(crate::runner::cloud::CloudIntent::Import);
            }
            SaveToCloud => {
                self.ensure_cloud_ready(crate::runner::cloud::CloudIntent::Save);
            }
            Save => {
                self.handle_save_file();
            }
            SaveCompleted(path) => self.handle_save_completed(path),
            SaveFailed(msg) => self.handle_save_failed(msg),
            SaveHintTimeout => {
                // 3 秒提示超时：恢复底边栏默认"就绪"
                self.window_state.window.ui_mut().set_status_message(None);
            }
            MidiLoaded(info) => {
                tracing::info!("MIDI 文件加载完成：{}", info);
            }
            MidiLoadError(err) => {
                tracing::error!("MIDI 文件加载失败：{}", err);
            }
            MidiParsed(parsed) => {
                tracing::info!("MIDI 文件解析完成：{}", parsed.info);

                // MIDI 加载后强制使用 Random 调色板并锁定（禁止用户修改）
                lumino_extras::palette::set_current_palette_by_name("Random");
                lumino_extras::palette::lock_palette();

                // 拆出所有权（事件传递路径上 Arc 唯一；极端情况有额外引用时
                // 浅拷贝 ParsedMidi——其 document 为 Arc 浅 clone，代价可忽略）
                let parsed = match std::sync::Arc::try_unwrap(parsed) {
                    Ok(parsed) => parsed,
                    Err(arc) => (*arc).clone(),
                };

                // 在 move 之前保存 info 相关数据（import 后 parsed.document 已移出）
                let source_path = std::path::PathBuf::from(&parsed.info.path);
                // 历史累计创作时间（.lmpj 工程文件跨会话累计）——必须在 parsed move 前提取
                let accumulated_editing_secs = parsed.accumulated_editing_secs;
                // 作者/版权（.lmpj 工程文件携带，常规 MIDI 为空）——必须在 parsed move 前提取，
                // 加载后回填工程设置对话框，修复"保存后重新打开显示空白"
                let project_author = parsed.author.clone();
                let project_copyright = parsed.copyright.clone();

                // 先导入音符到编辑器（新的懒加载模式：只加载当前音轨，其他音轨按需加载）
                self.import_midi_to_editor(parsed);

                tracing::debug!("MIDI 文档已导入编辑器（MidiDocument 已移入 UI 单一权威源）");

                self.log_memory_usage_after_import();

                // 保留 source 路径；document 已通过 import_midi_to_editor 移入 UI
                // （EditorData.document 独占），runner 不再持有文档副本，避免双份数据。
                self.midi_state.current_midi_source = Some(source_path);
                self.midi_state.current_midi = None;

                // 工程级数据随新文件加载一起归零：编辑计时/累计时间
                // （创建时间随后从文件系统重新设置；工程设置对话框的
                //   标题/作者/版权同样重置，防止上一工程的设置残留）
                self.session_tracker.reset();
                // 从 .lmpj 工程文件恢复历史累计创作时间（metadata.stats.working_time_seconds），
                // 常规 MIDI 文件加载时该值为 0——跨会话累计的关键一环。
                self.session_tracker.accumulated_editing_secs = accumulated_editing_secs;
                self.window_state.window.ui_mut().reset_project_settings();

                // 回填 .lmpj 携带的作者/版权到工程设置对话框状态
                // （reset 已清空，此处重新写入，关闭工程后重开面板显示正确值）
                self.window_state
                    .window
                    .ui_mut()
                    .set_project_author_and_copyright(project_author, project_copyright);

                // 设置工程创建时间（从文件系统获取）
                self.session_tracker.created_at = self
                    .midi_state
                    .current_midi_source
                    .as_ref()
                    .and_then(|p| format_created_at_from_path(p));
                if self.session_tracker.created_at.is_some() {
                    tracing::info!(
                        "工程创建时间已设置: {}",
                        self.session_tracker.created_at.as_deref().unwrap_or("")
                    );
                }

                if let Some(state) = &mut self.test_state.test_mode_state {
                    state.active = true;
                }

                // 加载完成后工程处于干净状态（无相对磁盘文件的未保存更改），
                // 标记清零避免误弹保存确认对话框。
                self.window_state.window.ui_mut().mark_project_clean();
            }
            MidiParseError(err) => {
                tracing::error!("MIDI 文件解析失败：{}", err);
                if self.test_state.test_mode_state.is_some() {
                    tracing::error!("测试模式因 MIDI 加载失败而退出");
                    event_loop.exit();
                }
            }
            Close => {
                // 关闭工程前检查未保存更改；有则弹保存确认对话框，无则直接关闭
                self.request_close_action(PendingCloseAction::CloseProject, event_loop);
            }
            ProjectSettings => {
                let saved_title = self.window_state.window.ui().get_project_settings_title();
                let display_title = if saved_title.is_empty() {
                    self.midi_state
                        .current_midi_source
                        .as_ref()
                        .and_then(|p| p.file_stem())
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "无标题".to_string())
                } else {
                    saved_title
                };
                let title = format!("{} - Lumino Midi", display_title);

                // 计算真实的创建时间和累计编辑时间
                let created_display = self.session_tracker.created_at.clone().unwrap_or_default();
                let total_editing_time_seconds = self.session_tracker.current_editing_secs();

                // 从编辑器获取当前 BPM 和拍号
                let (tempo, time_signatures, copyright, author) = {
                    let ui = self.window_state.window.ui();
                    let root = ui.root();
                    let tempo = root
                        .editor
                        .editor_state
                        .data
                        .tempo_points
                        .first()
                        .map(|tp| format!("{:.1}", tp.bpm))
                        .unwrap_or_else(|| "120.0".to_string());
                    let time_signatures = root.editor.editor_state.data.time_signatures.clone();
                    // 版权/作者来自对话框状态：同一工程内多次打开保留已填写的值；
                    // 跨工程（关闭/新建/加载新文件）时状态已重置为空，不会残留。
                    let copyright = ui.get_project_copyright();
                    let author = ui.get_project_author();
                    (tempo, time_signatures, copyright, author)
                };

                // 将真实数据设置到 UI 状态中
                self.window_state.window.ui_mut().set_project_settings_data(
                    lumino_ui::root::ProjectSettingsDialogData {
                        title: display_title.clone(),
                        tempo,
                        copyright,
                        author,
                        created_display,
                        total_editing_time_seconds,
                        time_signatures,
                    },
                );

                self.window_state
                    .dialog_manager
                    .open_project_settings(title);
            }
            Settings => {
                // 打开设置面板前刷新云存储快照（云管理页显示最新状态）
                self.refresh_cloud_connections();
                self.window_state
                    .dialog_manager
                    .open_dialog(crate::runner::dialog_manager::DialogType::Settings);
            }
            TrackSelected(track_idx) => {
                // 统一使用 cache-only 模式，只切换音轨索引
                // 播放时从 cache 流式读取，不单独加载音轨到编辑器
                tracing::info!("切换到音轨：{}", track_idx);
                self.window_state
                    .window
                    .ui_mut()
                    .set_current_track(track_idx, true);
            }
            ExportProjectArchive => {
                self.handle_export_project_archive();
            }
            ExportProjectFolder => {
                self.handle_export_project_folder();
            }
            ExportMaterial => {
                self.handle_export_material();
            }
            ExportMidi => {
                self.handle_export_midi();
            }
            _ => {
                tracing::debug!("未处理的文件事件：{:?}", file_event);
            }
        }
    }

    /// 导出当前编辑器内容为 MIDI 文件（.mid）
    ///
    /// 与"保存"的区别：只生成独立的 MIDI 文件，不修改 `current_midi_source`，
    /// 因此不会影响后续的 Ctrl+S 覆盖保存路径。
    pub(super) fn handle_export_midi(&mut self) {
        // 自动提交当前编辑（ghost 方案：松手即提交），
        // 保证导出的数据包含用户正在编辑（拖动/绘制/调整大小）的音符。
        let committed = self
            .window_state
            .window
            .ui_mut()
            .root_mut()
            .editor
            .commit_current_edit();
        if committed {
            tracing::debug!("导出 MIDI 前自动提交编辑");
        }

        // 从编辑器内容构建导出数据（无音符/文档时返回 None）
        let Some(export_data) = editor_midi::build_midi_export_data_from_editor(self, true) else {
            let language = self.window_state.window.ui().settings().display.language;
            let msg = lumino_extras::i18n::main_translations(language)
                .status_no_midi_content
                .to_string();
            self.window_state
                .window
                .ui_mut()
                .set_status_message(Some(msg));
            tracing::info!("导出 MIDI：没有可导出的内容");
            return;
        };

        let file_stem = self
            .midi_state
            .current_midi_source
            .as_ref()
            .map(|p| helpers::get_file_stem(std::path::Path::new(p)))
            .unwrap_or_else(|| "untitled".to_string());

        let Some(save_path) = rfd::FileDialog::new()
            .add_filter(
                crate::constants::filters::MIDI_FILES.0,
                crate::constants::filters::MIDI_FILES.1,
            )
            .set_file_name(format!("{file_stem}.mid"))
            .save_file()
        else {
            return;
        };

        match lumino_export::export_midi(&save_path, &export_data) {
            Ok(()) => {
                tracing::info!("MIDI 已导出: {:?}", save_path);
                let language = self.window_state.window.ui().settings().display.language;
                let msg = lumino_extras::i18n::main_translations(language)
                    .status_midi_exported
                    .to_string();
                self.window_state
                    .window
                    .ui_mut()
                    .set_status_message(Some(msg));
            }
            Err(e) => {
                tracing::error!("导出 MIDI 失败: {}", e);
                let language = self.window_state.window.ui().settings().display.language;
                let msg = format!(
                    "{}：{e}",
                    lumino_extras::i18n::main_translations(language).status_save_failed
                );
                self.window_state
                    .window
                    .ui_mut()
                    .set_status_message(Some(msg));
            }
        }
    }

    /// 创建新文件
    pub(super) fn handle_new_file(&mut self) {
        // 清空当前工程
        self.midi_state.current_midi = None;
        self.midi_state.current_midi_source = None;
        self.midi_state.cloud_source = None;
        lumino_extras::palette::unlock_palette();

        // 工程级数据随新工程一起归零：编辑计时/创建时间
        // （clear_editor 内部已重置工程设置对话框状态）
        self.session_tracker.reset();

        // 清空编辑器
        self.window_state.window.ui_mut().clear_editor();

        // 恢复主窗口默认标题（工程设置确认时设置的 "{标题} - Lumino"）
        self.window_state.window.window().set_title("Lumino");

        tracing::info!("已创建新工程");
    }

    /// 关闭当前工程（菜单「关闭」）
    ///
    /// 清空编辑器与工程级状态，恢复空白工程与默认标题。
    /// 未保存更改的检查在调用方（`request_close_action`）完成。
    pub(super) fn handle_close_project(&mut self) {
        self.midi_state.current_midi = None;
        self.midi_state.current_midi_source = None;
        self.midi_state.cloud_source = None;
        lumino_extras::palette::unlock_palette();
        self.window_state
            .window
            .ui_mut()
            .dispose_texture_waterfall();
        // 工程级数据必须随工程关闭一起归零：编辑计时/创建时间
        // （clear_editor 内部已重置工程设置对话框状态）
        self.session_tracker.reset();
        self.window_state.window.ui_mut().clear_editor();
        // 恢复主窗口默认标题（工程设置确认时设置的 "{标题} - Lumino"）
        self.window_state.window.window().set_title("Lumino");
        tracing::info!("工程已关闭");
    }

    // ── 保存确认对话框（关闭工程 / 打开另一个工程 / 退出前的未保存更改确认）────

    /// 请求执行一个关闭类动作，必要时先弹出保存确认对话框
    ///
    /// 若工程存在未保存更改，则暂存 `action` 并打开保存确认对话框；
    /// 否则立即执行。保存确认对话框已打开时忽略重复请求（避免堆叠）。
    pub(crate) fn request_close_action(
        &mut self,
        action: PendingCloseAction,
        event_loop: &ActiveEventLoop,
    ) {
        // 保存确认对话框已挂起（或正在初始化）：忽略重复触发，避免堆叠
        if self.pending_close_action.is_some()
            || self
                .window_state
                .dialog_manager
                .has_dialog_type(DialogType::SaveConfirm)
        {
            tracing::debug!("保存确认对话框已挂起，忽略重复关闭请求");
            return;
        }

        if self.window_state.window.ui().is_project_modified() {
            tracing::debug!("工程存在未保存更改，弹出保存确认对话框：{:?}", action);
            self.pending_close_action = Some(action);
            self.window_state
                .dialog_manager
                .open_dialog(DialogType::SaveConfirm);
        } else {
            self.execute_close_action(action, event_loop);
        }
    }

    /// 执行挂起的关闭类动作
    ///
    /// `Exit` / `WindowClose` 在保存进行中时转为 `close_pending`（保存完成后由
    /// `about_to_wait` 退出），否则直接退出事件循环；其余动作立即执行对应处理。
    fn execute_close_action(&mut self, action: PendingCloseAction, event_loop: &ActiveEventLoop) {
        match action {
            PendingCloseAction::CloseProject => self.handle_close_project(),
            PendingCloseAction::NewProject => self.handle_new_file(),
            PendingCloseAction::OpenProject => self.handle_open_file(),
            PendingCloseAction::Exit | PendingCloseAction::WindowClose => {
                // 保存期间禁止关闭：转为待关闭，保存完成后自动退出
                if self.is_saving() || self.is_cloud_saving() {
                    tracing::info!("保存进行中，退出请求延迟到保存完成");
                    self.window_state.window.close_pending = true;
                } else {
                    event_loop.exit();
                }
            }
        }
    }

    /// 处理保存确认对话框结果
    ///
    /// - 保存：记住待执行动作，保存完成后（`handle_save_completed`）继续；
    ///   若保存未实际启动（如空白工程无路径），直接继续避免卡死。
    /// - 关闭（放弃）：执行挂起的动作（Exit/WindowClose 转为 close_pending）。
    /// - 取消：清空挂起动作，继续编辑。
    pub(crate) fn handle_save_confirm_result(&mut self, choice: SaveConfirmAction) {
        match choice {
            SaveConfirmAction::Save => {
                let had_pending = self.pending_close_action.is_some();
                self.run_pending_after_save = had_pending;
                // 执行保存（异步；完成后 handle_save_completed 继续原动作）
                let started = self.handle_save_file();
                if started {
                    // 保存已启动：等待 handle_save_completed 继续挂起的关闭动作
                    tracing::info!("保存确认：已启动保存，等待保存完成后继续关闭动作");
                } else {
                    // 保存未启动（用户取消保存对话框 / 保存进行中被拒绝）：
                    // 视为取消关闭操作，清空挂起状态，绝不强制关闭工程。
                    tracing::info!("保存确认：保存未启动，放弃关闭操作");
                    self.pending_close_action = None;
                    self.run_pending_after_save = false;
                }
            }
            SaveConfirmAction::Discard => {
                // 放弃更改：直接执行挂起的关闭动作（Exit/WindowClose 转为 close_pending）
                self.run_pending_after_save = false;
                self.execute_pending_close_action();
            }
            SaveConfirmAction::Cancel => {
                // 取消关闭操作：清空挂起状态，继续编辑
                self.pending_close_action = None;
                self.run_pending_after_save = false;
                tracing::info!("保存确认：用户取消关闭操作");
            }
        }
    }

    /// 继续（或在放弃/保存完成后执行）挂起的关闭类动作
    ///
    /// 供「放弃更改」与「保存完成后」复用。`Exit` / `WindowClose` 转为 `close_pending`，
    /// 由 `about_to_wait` 在下一帧退出事件循环（无 `event_loop` 上下文时使用此路径）。
    fn execute_pending_close_action(&mut self) {
        let action = match self.pending_close_action.take() {
            Some(action) => action,
            None => return,
        };
        self.run_pending_after_save = false;
        match action {
            PendingCloseAction::CloseProject => self.handle_close_project(),
            PendingCloseAction::NewProject => self.handle_new_file(),
            PendingCloseAction::OpenProject => self.handle_open_file(),
            PendingCloseAction::Exit | PendingCloseAction::WindowClose => {
                self.window_state.window.close_pending = true;
            }
        }
    }

    /// 保存完成后继续挂起的关闭类动作（保存确认对话框选择「保存」时设置）
    fn finish_pending_close_action(&mut self) {
        self.execute_pending_close_action();
    }

    /// 保存完成：记录路径 + 底边栏提示（3 秒）+ 云端自动回传
    fn handle_save_completed(&mut self, path: std::path::PathBuf) {
        // 记录保存路径（后续 Ctrl+S 直接覆盖保存原文件）
        self.midi_state.current_midi_source = Some(path.clone());
        tracing::info!("工程保存完成，路径已记录：{:?}", path);

        // 底边栏显示"文件已经保存"，3 秒后自动恢复"就绪"
        let language = self.window_state.window.ui().settings().display.language;
        let saved_msg = lumino_extras::i18n::main_translations(language)
            .status_file_saved
            .to_string();
        self.window_state
            .window
            .ui_mut()
            .set_status_message(Some(saved_msg));
        self.spawn_status_hint_timeout();

        // 从云端打开的文件：自动上传回云端原路径
        // （若已有上传进行中，run_cloud_upload_overwrite 会直接拒绝本次回传，
        //   不排队不补传——用户可待上传完成后再次 Ctrl+S）
        if let Some(src) = self.midi_state.cloud_source.clone() {
            tracing::info!("工程来自云端，自动上传回原路径：{}", src.remote_path);
            self.run_cloud_upload_overwrite(src.conn_id, src.remote_path, path);
        }

        // 保存确认对话框选择「保存」后，保存完成即继续挂起的关闭动作
        // （关闭工程 / 打开另一个工程 / 退出）。无挂起动作时为空操作。
        if self.run_pending_after_save {
            self.finish_pending_close_action();
        }

        // 保存完成后工程处于干净状态（无未保存更改）
        self.window_state.window.ui_mut().mark_project_clean();
    }

    /// 保存失败：底边栏提示错误原因（3 秒后自动恢复）
    fn handle_save_failed(&mut self, msg: String) {
        tracing::error!("保存失败：{msg}");
        // 保存确认对话框选择「保存」但保存失败：放弃挂起的关闭动作，
        // 避免卡在半关闭状态或强制关闭未保存的工程。
        if self.run_pending_after_save {
            tracing::warn!("保存确认：保存失败，放弃挂起的关闭操作");
            self.pending_close_action = None;
            self.run_pending_after_save = false;
        }
        let language = self.window_state.window.ui().settings().display.language;
        let fail_msg = format!(
            "{}：{msg}",
            lumino_extras::i18n::main_translations(language).status_save_failed
        );
        self.window_state
            .window
            .ui_mut()
            .set_status_message(Some(fail_msg));
        self.spawn_status_hint_timeout();
    }

    /// 3 秒后发送提示超时事件，清除底边栏状态消息
    fn spawn_status_hint_timeout(&self) {
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            lumino_ui::event::emit(lumino_ui::event::Event::menu_file(
                lumino_ui::event::menu::file::Event::save_hint_timeout(),
            ));
        });
    }
}

/// 从文件路径读取文件创建时间并格式化为本地时间字符串
fn format_created_at_from_path(path: &std::path::Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let created = metadata.modified().ok()?;
    let since_epoch = created
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = since_epoch.as_secs() as i64;
    let datetime = chrono::DateTime::from_timestamp(secs, 0)
        .map(|dt| dt.with_timezone(&chrono::Local))
        .unwrap_or_else(chrono::Local::now);
    Some(datetime.format("%Y-%m-%d %H:%M:%S").to_string())
}
