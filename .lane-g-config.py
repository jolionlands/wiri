from pathlib import Path
exec(Path('.lane-g-edit.py').read_text().split("p='src/layout/workspace.rs'")[0])
p='src/layout/mod.rs'
edit(p,'pub mod snap;','pub mod snap;\npub mod strip;')
p='src/config/types.rs'
edit(p,'    pub column_width_mode: Option<String>,','''    pub column_width_mode: Option<String>,
    pub center_focused_column: Option<String>,
    pub always_center_single_column: Option<bool>,
    pub preset_column_widths: Option<Vec<SizeSpec>>,
    pub default_column_width: Option<Option<SizeSpec>>,''')
edit(p,'    pub column_width_mode: String,','''    pub column_width_mode: String,
    pub center_focused_column: String,
    pub always_center_single_column: bool,
    pub preset_column_widths: Vec<SizeSpec>,
    pub default_column_width: Option<Option<SizeSpec>>,''')
edit(p,'        if let Some(v) = self.strip_frame {','''        if let Some(ref v) = self.center_focused_column { out.center_focused_column = v.clone(); }
        if let Some(v) = self.always_center_single_column { out.always_center_single_column = v; }
        if let Some(ref v) = self.preset_column_widths { out.preset_column_widths = v.clone(); }
        if let Some(v) = self.default_column_width { out.default_column_width = Some(v); }
        if let Some(v) = self.strip_frame {''')
# Config defaults are assigned manually in Config::default (LayoutConfig derives default).
edit(p,'            column_width_mode: "proportional".to_string(),','''            column_width_mode: "fixed".to_string(),
            center_focused_column: "never".to_string(),
            always_center_single_column: false,
            preset_column_widths: Vec::new(),
            default_column_width: None,''')
for side in ['top','bottom','left','right']:
    edit(p,f'pub reserve_{side}: Option<u32>',f'pub reserve_{side}: Option<i32>')
    edit(p,f'pub reserve_{side}: u32',f'pub reserve_{side}: i32')
p='src/config/decode.rs'
start='''                        let value = match strut {
                            Some(v) => {
                                if v < 0.0 {
                                    self.warn(
                                        c.pos,
                                        "negative struts are not supported by wiri; using 0",
                                    );
                                    Some(0)
                                } else {
                                    Some(v.round() as u32)
                                }
                            }
                            None => None,
                        };'''
edit(p,start,'                        let value = strut.map(|v| v.round() as i32);')
for side in ['top','bottom','left','right']:
    edit(p,f'"reserve-{side}" => self.set_u32(child, 100_000, &mut p.reserve_{side}),',f'"reserve-{side}" => self.set_i32(child, &mut p.reserve_{side}),')
edit(p,'''                // niri layout options wiri does not implement yet
                "center-focused-column"
                | "always-center-single-column"
                | "empty-workspace-above-first"''','''                "center-focused-column" => {
                    if let Some(mode) = self.arg_text(child) {
                        if matches!(mode.as_str(), "never" | "always" | "on-overflow") {
                            p.center_focused_column = Some(mode);
                        } else {
                            self.error(child.pos, "expected never, always, or on-overflow");
                        }
                    }
                }
                "always-center-single-column" => self.set_flag(child, &mut p.always_center_single_column),
                "default-column-width" => {
                    if let Some(size) = self.decode_size_block(child) { p.default_column_width = Some(size); }
                }
                "preset-column-widths" => {
                    self.section(child);
                    let mut sizes = Vec::new();
                    for c in child.kids() {
                        match norm(&c.name).as_str() {
                            "proportion" => { if let Some(v) = self.arg_number(c, 0.0, 100.0) { sizes.push(SizeSpec::Proportion(v)); } }
                            "fixed" => { if let Some(v) = self.arg_number(c, 1.0, 100_000.0) { sizes.push(SizeSpec::Fixed(v.round() as u32)); } }
                            _ => self.unknown(c, "'preset-column-widths'", &["proportion", "fixed"]),
                        }
                    }
                    p.preset_column_widths = Some(sizes);
                }
                // niri layout options wiri does not implement yet
                "empty-workspace-above-first"''')
edit(p,'                | "preset-column-widths"\n                | "default-column-width"\n','')
p='src/layout/engine.rs'
edit(p,'use sizing::DrawMode;','use sizing::DrawMode;\nuse super::strip::CenterFocusedColumn;')
edit(p,'    pub column_width_mode: ColumnWidthMode,','''    pub column_width_mode: ColumnWidthMode,
    pub center_focused_column: CenterFocusedColumn,
    pub always_center_single_column: bool,
    pub preset_column_widths: Vec<crate::config::types::SizeSpec>,
    pub default_column_width: Option<Option<crate::config::types::SizeSpec>>,''')
edit(p,'            column_width_mode: ColumnWidthMode::Fixed,\n            max_visible_columns: 0,','''            column_width_mode: ColumnWidthMode::Fixed,
            center_focused_column: CenterFocusedColumn::Never,
            always_center_single_column: false,
            preset_column_widths: Vec::new(),
            default_column_width: None,
            max_visible_columns: 0,''')
edit(p,'        lc.strip_frame = config.layout.strip_frame;','''        lc.center_focused_column = match config.layout.center_focused_column.as_str() {
            "always" => CenterFocusedColumn::Always,
            "on-overflow" => CenterFocusedColumn::OnOverflow,
            _ => CenterFocusedColumn::Never,
        };
        lc.always_center_single_column = config.layout.always_center_single_column;
        lc.preset_column_widths = config.layout.preset_column_widths.clone();
        lc.default_column_width = config.layout.default_column_width;
        lc.strip_frame = config.layout.strip_frame;''')
for side in ['top','bottom','left','right']:
    edit(p,f'pub reserve_{side}: u32',f'pub reserve_{side}: i32')
# Different per-output settings must be respected by reveal.
edit(p,'''        let focus_col = self
            .monitors
            .get(&focused_output)
            .and_then(|m| m.focus_column);''','''        self.refresh_strip_widths(focused_output);
        let cfg = self.effective_config(focused_output).clone();
        let focus_col = self
            .monitors
            .get(&focused_output)
            .and_then(|m| m.focus_column);''')
edit(p,'Self::view_width_for_monitor(m, &self.config));\n        let default_width = match', 'Self::view_width_for_monitor(m, &cfg));\n        let default_width = match')
edit(p,'Self::workspace_default_width(w, vw, &self.config)),\n            _ => None,\n        };', 'Self::workspace_default_width(w, vw, &cfg)),\n            _ => None,\n        };')
edit(p,'''            let gap = self.config.column_gap;
            if let Some(monitor) = self.monitors.get_mut(&focused_output) {
                if let Some(workspace) = monitor.workspace_mut() {
                    workspace.reveal_column(col_idx, col_w, gap, vw);
                    // Keep the strip filling the view (no empty space at the right).
                    workspace.fit_scroll(col_w, gap, vw);
                }
            }''','''            let gap = cfg.column_gap;
            if let Some(monitor) = self.monitors.get_mut(&focused_output) {
                let focused = monitor.focus_window;
                if let Some(workspace) = monitor.workspace_mut() {
                    if let Some(id) = focused { workspace.remember_focus(id); }
                    workspace.reveal_with_policy(col_idx, col_w, gap, vw, cfg.center_focused_column, cfg.always_center_single_column);
                }
            }''')
edit(p,'    pub fn apply_layout_for_monitor(&mut self, output_id: OutputId, backend: &BackendHandle) {','''    pub fn apply_layout_for_monitor(&mut self, output_id: OutputId, backend: &BackendHandle) {
        self.refresh_strip_widths(output_id);''')
# Preserve % sizing intent rather than freezing it as pixels.
edit(p,'            column.set_width(Some(width));\n        }\n        self.reveal_focused_column(output);\n        self.apply_layout_for_monitor(output, backend);\n    }\n\n    /// The index', '''            match change {
                SizeChange::SetProportion(p) => column.set_proportion(p as f64 / 100_000.0, view, gap),
                _ => column.set_width(Some(width)),
            }
        }
        self.reveal_focused_column(output);
        self.apply_layout_for_monitor(output, backend);
    }

    /// The index''')
# Hook new column rule widths after admission; existing metadata covers width moves/expels.
edit(p,'        // A window known to have a minimum width (it floated, or was added again) gets it on', '''        self.remember_column_width_spec(window_id, rules.default_column_width);

        // A window known to have a minimum width (it floated, or was added again) gets it on''')
edit(p,'mod ipcview;','mod ipcview;\nmod strip_model;')
