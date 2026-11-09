//! Widget showcase: `cargo run -p termax-ui --release --example demo`
//! (sidebar with a 100k-row filterable list, tabs, form, context menu).

use termax_ui::demo::Demo;
use termax_ui::shell::{App, FrameCtx, WindowOptions, run};
use termax_ui::style::Style;
use termax_ui::text::Fonts;
use termax_ui::ui::{Ui, UiState};

struct DemoApp {
    state: UiState,
    style: Style,
    demo: Demo,
}

impl App for DemoApp {
    fn frame(&mut self, ctx: &mut FrameCtx) {
        let demo = &mut self.demo;
        Ui::run(&mut self.state, ctx, &self.style, |ui| demo.ui(ui));
    }
}

fn main() -> anyhow::Result<()> {
    let fonts = Fonts::load(None, None)?;
    let app = DemoApp { state: UiState::default(), style: Style::dark_default(), demo: Demo::default() };
    run(app, WindowOptions { title: "termax-ui demo".into(), size: (1100, 700), fonts })
}
