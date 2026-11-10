//! Widget showcase: `cargo run -p skiff-ui --release --example demo`
//! (sidebar with a 100k-row filterable list, tabs, form, context menu).

use skiff_ui::demo::Demo;
use skiff_ui::shell::{App, FrameCtx, WindowOptions, run};
use skiff_ui::style::Style;
use skiff_ui::text::Fonts;
use skiff_ui::ui::{Ui, UiState};

/// The demo screen wrapped as a shell `App`.
struct DemoApp {
    state: UiState,
    style: Style,
    demo: Demo,
}

impl App for DemoApp {
    /// Runs one `Ui` frame of the demo.
    fn frame(&mut self, ctx: &mut FrameCtx) {
        let demo = &mut self.demo;
        Ui::run(&mut self.state, ctx, &self.style, |ui| demo.ui(ui));
    }
}

/// Loads system fonts and opens the demo window.
fn main() -> anyhow::Result<()> {
    let fonts = Fonts::load(None, None)?;
    let app = DemoApp { state: UiState::default(), style: Style::dark_default(), demo: Demo::default() };
    run(app, WindowOptions { title: "skiff-ui demo".into(), app_id: "skiff-ui-demo".into(), size: (1100, 700), fonts, icon: None })
}
