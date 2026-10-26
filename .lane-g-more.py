from pathlib import Path
exec(Path('.lane-g-edit.py').read_text().split("p='src/layout/workspace.rs'")[0])
p='src/layout/workspace.rs'
edit(p,'    pub width_proportion: Option<f64>,','    pub width_proportion: Option<f64>,\n    pub width_fixed_logical: Option<f64>,')
edit(p,'            width_proportion: None,','            width_proportion: None,\n            width_fixed_logical: None,')
edit(p,'        self.width_proportion = None;','        self.width_proportion = None;\n        self.width_fixed_logical = None;')
edit(p,'    pub fn active_tile(&self)', '''    pub fn set_fixed_logical(&mut self, logical: f64, scale: f64, border: i32) {
        self.set_width(Some(((logical * scale).round() as i32 + border).clamp(50, 100_000) as u32));
        self.width_fixed_logical = Some(logical);
    }

    pub fn active_tile(&self)''')
edit(p,'    pub column_width_proportion: Option<f64>,','    pub column_width_proportion: Option<f64>,\n    pub column_width_fixed_logical: Option<f64>,')
edit(p,'            column_width_proportion: col.width_proportion,','            column_width_proportion: col.width_proportion,\n            column_width_fixed_logical: col.width_fixed_logical,')
edit(p,'        col.width_proportion = slot.column_width_proportion;','        col.width_proportion = slot.column_width_proportion;\n        col.width_fixed_logical = slot.column_width_fixed_logical;')
edit(p,'        new_col.width_proportion = source.width_proportion;','        new_col.width_proportion = source.width_proportion;\n        new_col.width_fixed_logical = source.width_fixed_logical;')
edit(p,'(self.columns[i].width, self.columns[i].width_proportion));','(self.columns[i].width, self.columns[i].width_proportion, self.columns[i].width_fixed_logical));')
edit(p,'if let Some((pixels, proportion)) = width {','if let Some((pixels, proportion, logical)) = width {')
edit(p,'                col.width_proportion = proportion;','                col.width_proportion = proportion;\n                col.width_fixed_logical = logical;')
# Unit tests for the shared signed area math and parser.
p='src/config/decode.rs'
edit(p,'    fn niri_layout_options_are_warnings()', '    fn niri_layout_options_are_warnings()') if False else None
