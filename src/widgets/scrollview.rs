use ratatui::widgets::StatefulWidget;

pub struct ScrollviewWidget;

impl StatefulWidget for ScrollviewWidget {
    type State = ();

    fn render(
        self,
        _area: ratatui::prelude::Rect,
        _buf: &mut ratatui::prelude::Buffer,
        _state: &mut Self::State,
    ) {
    }
}
