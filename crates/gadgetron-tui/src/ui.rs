use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn draw(f: &mut Frame) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(size);

    draw_header(f, chunks[0]);
    draw_body(f, chunks[1]);
    draw_footer(f, chunks[2]);
}

fn draw_header(f: &mut Frame, area: Rect) {
    let header = Paragraph::new(" Gadgetron Orchestrator ").style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(header, area);
}

fn draw_body(f: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    draw_nodes_panel(f, chunks[0]);
    draw_models_panel(f, chunks[1]);
}

fn draw_nodes_panel(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .title(" Nodes ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Green));
    f.render_widget(block, area);
}

fn draw_models_panel(f: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let models_block = Block::default()
        .title(" Models ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Yellow));
    f.render_widget(models_block, chunks[0]);

    let requests_block = Block::default()
        .title(" Recent Requests ")
        .borders(Borders::ALL)
        .style(Style::default().fg(Color::Blue));
    f.render_widget(requests_block, chunks[1]);
}

fn draw_footer(f: &mut Frame, area: Rect) {
    let footer = Paragraph::new(" q: quit | \u{2191}\u{2193}: navigate | r: refresh ")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, area);
}
