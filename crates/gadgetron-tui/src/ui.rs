use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::{App, FocusedPanel};

/// Top-level layout:
///
/// ```text
/// ┌─────────────────────────────────────────────┐  <- Header (3 rows)
/// ├──────────────┬──────────────┬───────────────┤
/// │  Nodes/GPU   │    Models    │   Requests    │  <- Body (Min)
/// ├──────────────┴──────────────┴───────────────┤
/// │  q/Esc: quit                                 │  <- Footer (3 rows)
/// └─────────────────────────────────────────────┘
/// ```
pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(0),    // body
            Constraint::Length(3), // footer
        ])
        .split(area);

    draw_header(f, rows[0], app);
    draw_body(f, rows[1], app);
    draw_footer(f, rows[2]);
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let health = app.health.read().unwrap();
    let text = format!(
        " Gadgetron Dashboard  Nodes: {}/{} | GPUs: {}/{} | Models: {} | RPS: {:.1} | Err: {:.1}%",
        health.healthy_nodes,
        health.total_nodes,
        health.active_gpus,
        health.total_gpus,
        health.models_loaded,
        health.requests_per_sec,
        health.error_rate_pct,
    );
    let header = Paragraph::new(text).style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(header, area);
}

fn draw_body(f: &mut Frame, area: Rect, app: &App) {
    // 3-column: Nodes (33%) / Models (33%) / Requests (34%)
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(34),
        ])
        .split(area);

    draw_nodes_panel(f, cols[0], app);
    draw_models_panel(f, cols[1], app);
    draw_requests_panel(f, cols[2], app);
}

fn draw_nodes_panel(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focused == FocusedPanel::Nodes;
    let scroll_offset = app.scroll.nodes;
    let metrics = app.gpu_metrics.read().unwrap();

    let border_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .title(" Nodes ")
        .borders(Borders::ALL)
        .border_style(border_style);

    if metrics.is_empty() {
        let msg = Paragraph::new("No nodes connected")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(block);
        f.render_widget(msg, area);
        return;
    }

    let visible_rows = area.height.saturating_sub(2) as usize;
    let items: Vec<ListItem> = metrics
        .iter()
        .skip(scroll_offset)
        .take(visible_rows)
        .map(|m| {
            let text = format!(
                "[{}] GPU{} {:.0}% VRAM:{:.1}G/{:.1}G {}C",
                m.node_id,
                m.gpu_index,
                m.utilization_pct,
                m.vram_used_mb as f64 / 1024.0,
                m.vram_total_mb as f64 / 1024.0,
                m.temperature_c,
            );
            // Color the row by temperature as primary signal; VRAM overrides to Red when critical.
            let color = temp_color(m.temperature_c);
            let vram_c = vram_color(m.vram_used_mb, m.vram_total_mb);
            // If VRAM is critical (Red), override to Red regardless of temperature.
            let final_color = if vram_c == Color::Red {
                Color::Red
            } else {
                color
            };
            ListItem::new(text).style(Style::default().fg(final_color))
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn draw_models_panel(f: &mut Frame, area: Rect, app: &App) {
    use gadgetron_core::ui::ModelState;

    let focused = app.focused == FocusedPanel::Models;
    let scroll_offset = app.scroll.models;
    let statuses = app.model_statuses.read().unwrap();

    let border_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .title(" Models ")
        .borders(Borders::ALL)
        .border_style(border_style);

    if statuses.is_empty() {
        let msg = Paragraph::new("No models loaded")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(block);
        f.render_widget(msg, area);
        return;
    }

    let visible_rows = area.height.saturating_sub(2) as usize;
    let items: Vec<ListItem> = statuses
        .iter()
        .skip(scroll_offset)
        .take(visible_rows)
        .map(|m| {
            let text = format!("[{}] {} {}", m.state, m.model_id, m.provider);
            let color = match m.state {
                ModelState::Running => Color::Green,
                ModelState::Loading | ModelState::Draining => Color::Yellow,
                ModelState::Stopped | ModelState::Error => Color::Red,
                #[allow(unreachable_patterns)]
                _ => Color::Gray,
            };
            ListItem::new(text).style(Style::default().fg(color))
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn draw_requests_panel(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focused == FocusedPanel::Requests;
    let scroll_offset = app.scroll.requests;
    let log = app.request_log.read().unwrap();

    let border_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .title(" Requests ")
        .borders(Borders::ALL)
        .border_style(border_style);

    if log.is_empty() {
        let msg = Paragraph::new("Waiting for requests...")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(block);
        f.render_widget(msg, area);
        return;
    }

    let visible_rows = area.height.saturating_sub(2) as usize;
    let items: Vec<ListItem> = log
        .iter()
        .rev() // newest request at top
        .skip(scroll_offset)
        .take(visible_rows)
        .map(|r| {
            let text = format!(
                "{} {} {}ms HTTP{}",
                r.request_id.get(..8).unwrap_or(&r.request_id),
                r.model,
                r.latency_ms,
                r.status,
            );
            // HTTP 4xx/5xx or explicit stream error status codes render as Red.
            let color = if r.status >= 400 {
                Color::Red
            } else {
                Color::Reset
            };
            ListItem::new(text).style(Style::default().fg(color))
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn draw_footer(f: &mut Frame, area: Rect) {
    let footer = Paragraph::new(" Tab: next panel  \u{2191}\u{2193}: scroll  q/Esc: quit ")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, area);
}

// ── Color helpers ──────────────────────────────────────────────────────────

/// Returns a ratatui `Color` based on GPU temperature (°C).
///
/// | Range      | Color         |
/// |------------|---------------|
/// | < 60 °C    | Green         |
/// | 60–74 °C   | Yellow        |
/// | 75–84 °C   | LightRed      |
/// | ≥ 85 °C    | Red           |
pub fn temp_color(t: f32) -> Color {
    if t < 60.0 {
        Color::Green
    } else if t < 75.0 {
        Color::Yellow
    } else if t < 85.0 {
        Color::LightRed
    } else {
        Color::Red
    }
}

/// Returns a ratatui `Color` based on VRAM utilization (used_mb / total_mb).
///
/// | Utilization | Color  |
/// |-------------|--------|
/// | < 70%       | Green  |
/// | 70–89%      | Yellow |
/// | ≥ 90%       | Red    |
///
/// Returns `Color::Green` when `total_mb == 0` to avoid division by zero.
pub fn vram_color(used_mb: u64, total_mb: u64) -> Color {
    if total_mb == 0 {
        return Color::Green;
    }
    let pct = used_mb as f32 / total_mb as f32 * 100.0;
    if pct < 70.0 {
        Color::Green
    } else if pct < 90.0 {
        Color::Yellow
    } else {
        Color::Red
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── temp_color ───────────────────────────────────────────────────────

    #[test]
    fn temp_color_below_60_is_green() {
        assert_eq!(temp_color(0.0), Color::Green);
        assert_eq!(temp_color(59.9), Color::Green);
    }

    #[test]
    fn temp_color_60_to_74_is_yellow() {
        assert_eq!(temp_color(60.0), Color::Yellow);
        assert_eq!(temp_color(74.9), Color::Yellow);
    }

    #[test]
    fn temp_color_75_to_84_is_light_red() {
        assert_eq!(temp_color(75.0), Color::LightRed);
        assert_eq!(temp_color(84.9), Color::LightRed);
    }

    #[test]
    fn temp_color_85_and_above_is_red() {
        assert_eq!(temp_color(85.0), Color::Red);
        assert_eq!(temp_color(110.0), Color::Red);
    }

    // ── vram_color ───────────────────────────────────────────────────────

    #[test]
    fn vram_color_zero_total_is_green() {
        assert_eq!(vram_color(0, 0), Color::Green);
    }

    #[test]
    fn vram_color_below_70pct_is_green() {
        // 69% = 6900/10000
        assert_eq!(vram_color(6_900, 10_000), Color::Green);
        assert_eq!(vram_color(0, 10_000), Color::Green);
    }

    #[test]
    fn vram_color_70_to_89pct_is_yellow() {
        // 70% = 7000/10000
        assert_eq!(vram_color(7_000, 10_000), Color::Yellow);
        // 89% = 8900/10000
        assert_eq!(vram_color(8_900, 10_000), Color::Yellow);
    }

    #[test]
    fn vram_color_90pct_and_above_is_red() {
        // 90% = 9000/10000
        assert_eq!(vram_color(9_000, 10_000), Color::Red);
        assert_eq!(vram_color(10_000, 10_000), Color::Red);
    }
}
