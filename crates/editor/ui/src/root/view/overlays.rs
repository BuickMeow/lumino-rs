//! 覆盖层/弹窗视图渲染函数
//!
//! 包含进度窗口和对话框窗口的渲染。

use iced_core::Length;
use iced_widget::{column, container, progress_bar, space, text};

use crate::root::Root;
use crate::state::root_state::DialogType;
use crate::view::{
    batch_edit_dialog::view_batch_edit_dialog, brush_settings_dialog::view_brush_settings_dialog,
    collaboration_dialog::view_collaboration_dialog,
    custom_precision_dialog::view_custom_precision_dialog,
    export_progress_dialog::view_export_progress_dialog,
    load_confirm_dialog::view_load_confirm_dialog,
    memory_monitor_dialog::view_memory_monitor_dialog,
    project_settings_dialog::view_project_settings_dialog,
    recover_track_dialog::view_recover_track_dialog, save_confirm_dialog::view_save_confirm_dialog,
    settings_dialog::view_settings_dialog, speed_change_dialog::view_speed_change_dialog,
    video_export_dialog::view_video_export_overlay,
};
use crate::{Element, Theme};

impl Root {
    /// 渲染进度窗口
    pub(super) fn view_progress(&self) -> Element<'_> {
        puffin::profile_scope!("root_view_progress");

        // 进度窗口只显示进度
        // 默认显示初始化状态，避免窗口空白
        let (msg, progress) = self
            .progress
            .as_ref()
            .map(|(m, p)| (m.as_str(), *p))
            .unwrap_or(("正在初始化...", 0.0));

        let content: Element<'_> = container(
            column![
                text("处理中...")
                    .size(24)
                    .style(|theme: &Theme| text::Style {
                        color: Some(theme.extended_palette().background.neutral.text),
                    }),
                text(msg).size(16).style(|theme: &Theme| text::Style {
                    color: Some(theme.extended_palette().background.neutral.text),
                }),
                progress_bar(0.0..=1.0, progress as f32),
            ]
            .spacing(20)
            .align_x(iced_core::Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(30)
        .style(|theme: &Theme| container::Style {
            background: Some(iced_core::Background::Color(theme.palette().background)),
            ..Default::default()
        })
        .into();

        if self.use_native_titlebar {
            content
        } else {
            column![self.titlebar.view_popup(&self.window), content]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    }

    /// 渲染对话框
    pub(super) fn view_dialog(&self) -> Element<'_> {
        puffin::profile_scope!("root_view_dialog");

        // 对话框窗口 - 根据类型显示不同内容
        let content: Element<'_> = match self.state.dialog_type {
            DialogType::Collaboration => {
                view_collaboration_dialog(&self.state.collaboration_dialog, &self.window.theme)
            }
            DialogType::LoadConfirm => {
                view_load_confirm_dialog(&self.state.load_confirm_dialog, &self.window.theme)
            }
            DialogType::ProjectSettings => view_project_settings_dialog(
                &self.state.project_settings_dialog,
                &self.window.theme,
                self.settings.display.language,
            ),
            DialogType::Settings => {
                // 字体列表仅在设置对话框中使用（字体下拉菜单），
                // 通过全局 OnceLock 懒加载：首次访问触发扫描并缓存，后续零开销。
                // 非设置对话框不走此路径，完全不受字体扫描影响。
                view_settings_dialog(
                    &self.settings,
                    &self.window,
                    lumino_note_core::font_scanner::get_cached_fonts(),
                )
            }
            DialogType::SpeedChange => {
                view_speed_change_dialog(&self.state.speed_change_dialog, &self.window.theme)
            }
            DialogType::BatchEdit => {
                view_batch_edit_dialog(&self.state.batch_edit_dialog, &self.window.theme)
            }
            DialogType::CustomPrecision => view_custom_precision_dialog(
                &self.state.custom_precision_dialog,
                &self.window.theme,
                self.settings.display.language,
            ),
            // DialogType::None: 关闭过程中 dialog_type 可能被复位为 None，
            // 此时渲染空容器避免闪跳到精度面板。实际关闭由 DialogWindow 的 should_close 驱动。
            DialogType::None => container(space())
                .width(iced_core::Length::Fill)
                .height(iced_core::Length::Fill)
                .style(|theme: &Theme| container::Style {
                    background: Some(iced_core::Background::Color(theme.palette().background)),
                    ..Default::default()
                })
                .into(),
            DialogType::ExportProgress => {
                view_export_progress_dialog(&self.state.export_progress_dialog, &self.window.theme)
            }
            DialogType::VideoExport => {
                // 对话框是进度窗口：overlay != None 时渲染进度覆盖层
                // overlay == None 时显示"准备中..."占位（不应发生，创建时已设为 Exporting）
                if let Some(overlay) =
                    view_video_export_overlay(&self.state.video_export_dialog, &self.window.theme)
                {
                    overlay
                } else {
                    container(
                        column![
                            text("准备中...").size(16),
                            space().height(20),
                            progress_bar(0.0..=1.0, 0.0),
                        ]
                        .spacing(12)
                        .align_x(iced_core::Alignment::Center),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(40)
                    .style(|theme: &Theme| container::Style {
                        background: Some(iced_core::Background::Color(theme.palette().background)),
                        ..Default::default()
                    })
                    .into()
                }
            }
            DialogType::MemoryMonitor => {
                view_memory_monitor_dialog(&self.state.memory_monitor_dialog, &self.window.theme)
            }
            DialogType::RecoverTrack => {
                view_recover_track_dialog(&self.state.recover_track_dialog, &self.window.theme)
            }
            DialogType::SaveConfirm => {
                view_save_confirm_dialog(&self.state.save_confirm_dialog, &self.window.theme)
            }
            DialogType::BrushSettings => view_brush_settings_dialog(
                &self.state.brush_settings_draft,
                &self.state.brush_settings_tracks,
                &self.window.theme,
                self.settings.display.language,
            ),
            // 云存储连接面板 / 云文件浏览面板（Phase 3/4 实现完整 UI）
            DialogType::CloudConnect | DialogType::CloudBrowser | DialogType::CloudNotice => {
                crate::view::cloud_dialog::view_cloud_dialog(self, &self.window.theme)
            }
        };

        if self.use_native_titlebar {
            content
        } else {
            column![
                self.titlebar.view_popup(&self.window),
                container(content).width(Length::Fill).height(Length::Fill),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        }
    }
}
