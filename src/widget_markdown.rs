use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

pub struct MdWidget {
    pub node: markdown::mdast::Node,
}

impl Widget for MdWidget {
    fn render(self, _area: Rect, _buf: &mut Buffer) {}
}

pub fn fmt_md_node() -> () {}
