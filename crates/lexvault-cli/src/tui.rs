use std::{io, time::Duration};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use lexvault_core::{Entry, LexVault, SearchCursor, Suggestion};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

const PAGE_SIZE: usize = 40;
const OVERSCAN: usize = 8;

pub fn run(dictionary: &LexVault) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, dictionary);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

#[derive(Default)]
struct TuiState {
    query: String,
    items: Vec<Suggestion>,
    cursor: Option<SearchCursor>,
    exhausted: bool,
    viewport_rows: usize,
    selected: usize,
    entry: Option<Entry>,
    status: String,
    detail_scroll: u16,
}

impl TuiState {
    fn refresh(&mut self, dictionary: &LexVault, viewport_rows: usize) {
        self.entry = None;
        self.detail_scroll = 0;
        self.selected = 0;
        self.items.clear();
        self.cursor = None;
        self.exhausted = false;
        self.viewport_rows = viewport_rows;
        if self.query.trim().is_empty() {
            self.exhausted = true;
            self.status.clear();
            return;
        }
        self.load_until(dictionary, viewport_rows.saturating_add(OVERSCAN));
        if !self.items.is_empty() {
            self.open_selected(dictionary);
        }
    }

    fn load_until(&mut self, dictionary: &LexVault, target: usize) {
        while self.items.len() < target && !self.exhausted {
            let page_size = target.saturating_sub(self.items.len()).max(PAGE_SIZE);
            match dictionary.search_page(&self.query, self.cursor.as_ref(), page_size) {
                Ok(page) => {
                    let before = self.items.len();
                    self.items.extend(page.items);
                    self.cursor = page.next_cursor;
                    self.exhausted = page.exhausted;
                    if self.items.len() == before && !page.exhausted {
                        self.status = "分页查询没有取得新词条".to_owned();
                        self.exhausted = true;
                    }
                }
                Err(error) => {
                    self.status = error.to_string();
                    self.exhausted = true;
                    break;
                }
            }
        }
    }

    fn open_selected(&mut self, dictionary: &LexVault) {
        self.status = "正在查询...".to_owned();
        let result = if let Some(suggestion) = self.items.get(self.selected) {
            dictionary.lookup_by_id(suggestion.id)
        } else {
            dictionary
                .lookup(&self.query)
                .map(|result| result.map(|result| result.entry))
        };
        match result {
            Ok(Some(entry)) => {
                self.entry = Some(entry);
                self.detail_scroll = 0;
                self.status.clear();
            }
            Ok(None) => {
                self.entry = None;
                self.status = format!("没有找到：{}", self.query);
            }
            Err(error) => {
                self.entry = None;
                self.status = error.to_string();
            }
        }
    }

    fn move_up(&mut self, dictionary: &LexVault) {
        self.selected = self.selected.saturating_sub(1);
        self.open_selected(dictionary);
    }

    fn move_down(&mut self, dictionary: &LexVault) {
        if self.items.is_empty() {
            self.open_selected(dictionary);
            return;
        }
        if self.selected + 1 >= self.items.len() && !self.exhausted {
            let target = self
                .items
                .len()
                .saturating_add(self.viewport_rows.max(1))
                .saturating_add(OVERSCAN);
            self.load_until(dictionary, target);
        }
        if self.selected + 1 < self.items.len() {
            self.selected += 1;
        }
        self.open_selected(dictionary);
    }
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    dictionary: &LexVault,
) -> io::Result<()> {
    let mut state = TuiState::default();
    loop {
        let viewport_rows = terminal.size()?.height.saturating_sub(7).max(1) as usize;
        if viewport_rows > state.viewport_rows && !state.query.trim().is_empty() {
            state.viewport_rows = viewport_rows;
            state.load_until(dictionary, viewport_rows.saturating_add(OVERSCAN));
        }
        terminal.draw(|frame| draw(frame, &state))?;
        if !event::poll(Duration::from_millis(150))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(());
        }
        match key.code {
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.query.push(ch);
                state.refresh(dictionary, viewport_rows);
            }
            KeyCode::Backspace => {
                state.query.pop();
                state.refresh(dictionary, viewport_rows);
            }
            KeyCode::Up => state.move_up(dictionary),
            KeyCode::Down => state.move_down(dictionary),
            KeyCode::Esc => {
                state.query.clear();
                state.refresh(dictionary, viewport_rows);
            }
            KeyCode::PageUp => {
                state.detail_scroll = state.detail_scroll.saturating_sub(8);
            }
            KeyCode::PageDown => {
                state.detail_scroll = state.detail_scroll.saturating_add(8);
            }
            _ => {}
        }
    }
}

fn draw(frame: &mut Frame<'_>, state: &TuiState) {
    let layout = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .split(frame.area());

    let search = Paragraph::new(Line::from(vec![
        Span::styled("搜索：", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(state.query.as_str()),
    ]))
    .block(Block::default().borders(Borders::ALL).title("LexVault"));
    frame.render_widget(search, layout[0]);

    let columns = Layout::horizontal([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(layout[1]);

    let items = state
        .visible_items()
        .iter()
        .map(|suggestion| {
            let detail = if suggestion.brief.is_empty() {
                String::new()
            } else {
                format!("  {}", truncate(&suggestion.brief, 28))
            };
            ListItem::new(Line::from(format!("{}{}", suggestion.word, detail)))
        })
        .collect::<Vec<_>>();
    let title = if state.query.trim().is_empty() {
        "候选".to_owned()
    } else if state.exhausted {
        format!("候选（已加载 {}）", state.items.len())
    } else {
        format!("候选（已加载 {}+）", state.items.len())
    };
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut list_state = ListState::default();
    if let Some((start, end)) = state.visible_range()
        && state.selected >= start
        && state.selected < end
    {
        list_state.select(Some(state.selected - start));
    }
    frame.render_stateful_widget(list, columns[0], &mut list_state);

    let detail = Paragraph::new(entry_lines(state.entry.as_ref()))
        .block(Block::default().borders(Borders::ALL).title("释义"))
        .wrap(Wrap { trim: false })
        .scroll((state.detail_scroll, 0));
    frame.render_widget(detail, columns[1]);

    let help = if state.status.is_empty() {
        "↑↓ 选择并查看释义  Esc 清空  PgUp/PgDn 滚动  Ctrl+C 退出"
    } else {
        state.status.as_str()
    };
    let footer = Paragraph::new(help)
        .style(Style::default().fg(if state.status.is_empty() {
            Color::DarkGray
        } else {
            Color::Red
        }))
        .block(Block::default().borders(Borders::TOP));
    frame.render_widget(footer, layout[2]);
}

impl TuiState {
    fn visible_range(&self) -> Option<(usize, usize)> {
        if self.items.is_empty() {
            return None;
        }
        let rows = self.viewport_rows.max(1).min(self.items.len());
        let half = rows / 2;
        let mut start = self.selected.saturating_sub(half);
        start = start.min(self.items.len().saturating_sub(rows));
        Some((start, (start + rows).min(self.items.len())))
    }

    fn visible_items(&self) -> &[Suggestion] {
        self.visible_range()
            .map(|(start, end)| &self.items[start..end])
            .unwrap_or_default()
    }
}

fn entry_lines(entry: Option<&Entry>) -> Vec<Line<'static>> {
    let Some(entry) = entry else {
        return vec![Line::from("输入单词开始查询")];
    };
    let mut lines = vec![Line::from(Span::styled(
        entry.word.clone(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))];
    if let Some(phonetic) = entry
        .phonetic
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(Line::from(format!("/{phonetic}/")));
    }
    if let Some(translation) = entry
        .translation
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(Line::from(""));
        lines.extend(translation.lines().map(|line| Line::from(line.to_owned())));
    }
    if let Some(definition) = entry
        .definition
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(Line::from(""));
        lines.extend(definition.lines().map(|line| Line::from(line.to_owned())));
    }
    if let Some(exchange) = entry
        .exchange
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("词形：{exchange}")));
    }
    lines
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let shortened = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{shortened}…")
    } else {
        shortened
    }
}
