//! The command palette, drawn over the window from what fzt says to draw:
//! its search line and the menu's rows. Or, in the same box, why it
//! couldn't open, or the info panel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::{border, bordered, centered, fit, hint_spans, pad};
use crate::app::App;
use crate::fzt;
use crate::palette::Palette;
use crate::text::cols;
use crate::theme::Colors;

/// The palette's box in a window of `area`.
pub fn area(area: Rect) -> Rect {
    centered(
        area,
        area.width.saturating_sub(4).min(100),
        area.height.saturating_sub(2).min(26),
    )
}

/// The rows' part of the box: below its border, the search line and a
/// blank line.
pub fn rows_area(area: Rect) -> Rect {
    let inner = bordered().inner(self::area(area));
    Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    }
}

pub fn draw(frame: &mut Frame, app: &App, window: Rect) {
    let Some(palette) = &app.palette else {
        return;
    };
    let c = &app.colors;
    let popup = area(window);
    let (title, hint) = match palette {
        Palette::Open { .. } => (
            " Commands ",
            " `Enter` run · `→` into a folder · `←` out · `Esc` close ",
        ),
        Palette::Failed(_) => (" Commands ", " any key closes "),
        Palette::Info(_) => (" tuimail ", " any key closes "),
    };
    frame.render_widget(Clear, popup);
    let block = bordered()
        .border_style(border(true, c))
        .title(Span::styled(title, Style::new().fg(c.accent)))
        .title_bottom(Line::from(hint_spans(hint, c)).right_aligned())
        .style(Style::new().bg(c.panel).fg(c.text));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    match palette {
        Palette::Open { frame: fzt, .. } => open(frame, fzt, inner, c),
        Palette::Failed(why) => {
            let text = format!(
                "The command palette couldn't open.\n\n{why}\n\nEvery command still has its key: ? lists them."
            );
            frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
        }
        Palette::Info(lines) => info(frame, lines, inner, c),
    }
}

fn open(frame: &mut Frame, fzt: &fzt::Frame, inner: Rect, c: &Colors) {
    let width = usize::from(inner.width);
    let dim = Style::new().fg(c.dim);
    let prompt = &fzt.prompt;
    let mut spans = vec![Span::styled(" › ", Style::new().fg(c.accent))];
    for scope in prompt.scope_path.iter().flatten() {
        spans.push(Span::styled(format!("{scope} › "), dim));
    }
    if prompt.query.is_empty() {
        let hint = if prompt.hint.is_empty() {
            "type to find a command"
        } else {
            &prompt.hint
        };
        spans.push(Span::styled(hint.to_string(), dim));
    } else {
        spans.push(Span::raw(prompt.query.clone()));
        spans.push(Span::styled(prompt.ghost.clone(), dim));
    }
    let line = Rect { height: 1, ..inner };
    frame.render_widget(Line::from(spans), line);

    let rows = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let name_width = fzt
        .rows
        .iter()
        .map(|r| r.depth * 2 + 2 + cols(&r.name))
        .max()
        .unwrap_or(0)
        .min(width / 2)
        + 2;
    let height = usize::from(rows.height);
    let selected = fzt.rows.iter().position(|r| r.is_selected).unwrap_or(0);
    let skip = (selected + 1).saturating_sub(height);
    let matched = Style::new().fg(c.accent).add_modifier(Modifier::BOLD);
    let lines: Vec<Line> = fzt
        .rows
        .iter()
        .skip(skip)
        .take(height)
        .map(|row| {
            let lead = format!(
                " {}{}",
                "  ".repeat(row.depth),
                if row.is_folder { "▸ " } else { "  " }
            );
            let room = name_width.saturating_sub(cols(&lead));
            let name = pad(&row.name, room);
            let description = fit(&row.description, width.saturating_sub(name_width + 1));
            let mut spans = vec![Span::raw(lead)];
            spans.extend(lit(
                &name,
                row.name_match_indices.as_deref(),
                Style::new().fg(c.text),
                matched,
            ));
            spans.extend(lit(
                &description,
                row.desc_match_indices.as_deref(),
                dim,
                matched,
            ));
            let line = Line::from(spans);
            if row.is_selected {
                line.style(Style::new().bg(c.selected))
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), rows);
}

/// `text` as spans, the characters at `indices` (fzt's matches) lit.
fn lit(text: &str, indices: Option<&[usize]>, style: Style, matched: Style) -> Vec<Span<'static>> {
    let indices = indices.unwrap_or_default();
    text.chars()
        .enumerate()
        .map(|(i, ch)| {
            let style = if indices.contains(&i) { matched } else { style };
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

fn info(frame: &mut Frame, lines: &[(&'static str, String)], inner: Rect, c: &Colors) {
    let label_width = lines.iter().map(|(l, _)| cols(l)).max().unwrap_or(0) + 2;
    let room = usize::from(inner.width).saturating_sub(label_width + 1);
    let lines: Vec<Line> = lines
        .iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(
                    format!(" {}", pad(label, label_width)),
                    Style::new().fg(c.dim),
                ),
                Span::styled(fit(value, room), Style::new().fg(c.text)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::app;
    use crate::palette::Palette;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| super::super::draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn the_palette_draws_fzts_rows_with_the_picked_one_lit() {
        let mut app = app();
        let frame = fzt::Frame {
            rows: vec![
                fzt::Row {
                    name: "mail".into(),
                    is_folder: true,
                    ..Default::default()
                },
                fzt::Row {
                    name: "reply".into(),
                    description: "r  ·  reply to the sender".into(),
                    depth: 1,
                    is_selected: true,
                    name_match_indices: Some(vec![0, 1]),
                    ..Default::default()
                },
            ],
            prompt: fzt::PromptState {
                query: "re".into(),
                ghost: "ply".into(),
                ..Default::default()
            },
            result: None,
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|f| open(f, &frame, Rect::new(0, 0, 80, 10), &app.colors))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let line = |y: u16| {
            (0..80)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        };
        assert!(line(0).starts_with(" › reply"), "{:?}", line(0));
        assert!(line(2).starts_with(" ▸ mail"), "{:?}", line(2));
        assert!(line(3).contains("reply") && line(3).contains("reply to the sender"));
        assert_eq!(buffer[(5, 3)].bg, app.colors.selected);
        assert_eq!(
            buffer[(5, 3)].fg,
            app.colors.accent,
            "a matched letter is lit"
        );
        app.palette = Some(Palette::Failed("fzt isn't installed".into()));
        let shown = screen(&mut app);
        assert!(shown.contains("couldn't open") && shown.contains("fzt isn't installed"));
    }

    #[tokio::test]
    async fn the_info_panel_names_the_repo_and_build() {
        let mut app = app();
        app.palette = Some(Palette::Info(vec![
            ("Repo", "romaine-life/tuimail".into()),
            ("Built", "a development build, not from CI".into()),
        ]));
        let shown = screen(&mut app);
        assert!(shown.contains("Repo   romaine-life/tuimail"), "{shown}");
        assert!(shown.contains("Built  a development build, not from CI"));
    }
}
