use super::ArrangementRenderer;

impl ArrangementRenderer {
    /// 运行音符 GPU 裁剪计算 pass（在渲染 pass 之前调用，需 `encoder`）
    ///
    /// 全量一次分发覆盖整份共享缓冲：cull 着色器按泳道映射 + 视口范围判定可见性，
    /// 输出可见实例的全局源索引到 `note_visible_buffer`，并以原子方式累加
    /// `note_indirect_buffer` 的 `instance_count`。绘制阶段据此 `draw_indirect`
    /// 一次性提交，CPU 零参与（消除此前每帧 ~67ms 的 CPU 逐音符重建）。
    pub fn run_cull(&self, encoder: &mut wgpu::CommandEncoder) {
        puffin::profile_scope!("arrangement::cull");
        let Some(cull_bg) = self.note_cull_bind_group.as_ref() else {
            return;
        };
        if self.note_instance_count == 0 {
            return;
        }

        const WORKGROUP_SIZE: u32 = 256;
        const MAX_DISPATCH_X: u32 = 65535;
        let workgroup_count = self.note_instance_count.div_ceil(WORKGROUP_SIZE);
        let dispatch_x = workgroup_count.min(MAX_DISPATCH_X);
        let dispatch_y = workgroup_count.div_ceil(MAX_DISPATCH_X);

        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("arrangement_note_cull_pass"),
            timestamp_writes: None,
        });
        compute_pass.set_pipeline(&self.note_cull_pipeline);
        compute_pass.set_bind_group(0, cull_bg, &[]);
        compute_pass.dispatch_workgroups(dispatch_x, dispatch_y, 1);
    }

    /// 绘制走带视图
    ///
    /// 绘制顺序：覆盖层背景（背景/lane/网格）→ 音符（GPU 裁剪结果 + `draw_indirect`）→
    /// 覆盖层前景（框选/ghost/指示线），保证音符位于 lane 之上、指示线位于音符之上。
    pub fn draw<'r>(&'r self, render_pass: &mut wgpu::RenderPass<'r>) {
        puffin::profile_scope!("arrangement::draw");
        if self.overlay_count == 0 && self.note_draw_bind_group.is_none() {
            return;
        }

        // ── 覆盖层背景（背景/lane/网格）──
        if self.overlay_back_len > 0 {
            render_pass.set_pipeline(&self.pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.overlay_buffer.inner().slice(..));
            render_pass.draw(0..6, 0..self.overlay_back_len);
        }

        // ── 音符（GPU 裁剪结果，draw_indirect 一次性提交）──
        if let Some(draw_bg) = self.note_draw_bind_group.as_ref() {
            render_pass.set_pipeline(&self.note_pipeline);
            render_pass.set_bind_group(0, draw_bg, &[]);
            render_pass.set_vertex_buffer(0, self.note_visible_buffer.inner().slice(..));
            render_pass.draw_indirect(self.note_indirect_buffer.inner(), 0);
        }

        // ── 覆盖层前景（框选/ghost/指示线）──
        let front_start = self.overlay_back_len;
        let front_end = self.overlay_count;
        if front_end > front_start {
            render_pass.set_pipeline(&self.pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.overlay_buffer.inner().slice(..));
            render_pass.draw(0..6, front_start..front_end);
        }
    }
}
