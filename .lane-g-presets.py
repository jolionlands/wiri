from pathlib import Path
exec(Path('.lane-g-edit.py').read_text().split("p='src/layout/workspace.rs'")[0])
p='src/ipc/server.rs'
for side in ['top','bottom','left','right']:
    edit(p,f'new_config.reserve_{side} = pixels,',f'new_config.reserve_{side} = pixels.min(i32::MAX as u32) as i32,')
p='src/layout/engine.rs'
edit(p,'mod strip_model;','mod strip_model;\nmod presets;')
edit(p,'''        let resolved: Vec<i32> = Self::PRESET_COLUMN_PROPORTIONS
            .iter()
            .map(|p| proportion_px(view, gap, *p))
            .collect();''','''        let cfg = self.effective_config(output).clone();
        let scale = self.monitors.get(&output).map_or(1.0, |m| m.scale_factor);
        let presets = self.column_presets(output);
        let resolved: Vec<i32> = presets.iter().map(|spec| match spec {
            crate::config::types::SizeSpec::Proportion(p) => proportion_px(view, gap, *p),
            crate::config::types::SizeSpec::Fixed(px) => (*px as f64 * scale).round() as i32 + 2 * cfg.border_width.max(1),
        }).collect();''')
edit(p,'''            column.set_width(Some(width));
            column.preset_idx = Some(idx);''','''            match presets[idx] {
                crate::config::types::SizeSpec::Proportion(p) => column.set_proportion(p, view, gap),
                crate::config::types::SizeSpec::Fixed(px) => column.set_fixed_logical(px as f64, scale, 2 * cfg.border_width.max(1)),
            }
            column.width = Some(width);
            column.preset_idx = Some(idx);''')
# Floats use the same configured list, but fixed widths have no tile border.
a='''        let presets = Self::PRESET_COLUMN_PROPORTIONS;
        let Some((output, _)) = self.float_location(window_id) else'''
edit(p,a,'''        let Some((output, _)) = self.float_location(window_id) else''')
a='''        let current = rect.size.w as f64;
        let wide = |f: f64| work.size.w as f64 * f;
        let next = if forwards {
            presets
                .iter()
                .copied()
                .find(|f| wide(*f) > current + 1.0)
                .unwrap_or(presets[0])
        } else {
            presets
                .iter()
                .rev()
                .copied()
                .find(|f| wide(*f) + 1.0 < current)
                .unwrap_or(presets[presets.len() - 1])
        };
        self.float_set_width_fraction(window_id, next, backend);'''
edit(p,a,'''        let scale = self.monitors.get(&output).map_or(1.0, |m| m.scale_factor);
        let widths: Vec<i32> = self.column_presets(output).iter().map(|spec| match spec {
            crate::config::types::SizeSpec::Proportion(p) => (work.size.w as f64 * p).round() as i32,
            crate::config::types::SizeSpec::Fixed(px) => (*px as f64 * scale).round() as i32,
        }).collect();
        let next = Self::next_preset_index(None, rect.size.w as i32, &widths, forwards);
        self.place_float(window_id, Rect::new(rect.loc.x, rect.loc.y, widths[next].max(50) as u32, rect.size.h), backend);''')
