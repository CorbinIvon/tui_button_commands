use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame, Terminal,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{self, Read, Result, Write},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use ui_framework::{
    components::{
        button::{Button, ButtonStyle},
        input::TextInput,
        text_button::TextButton,
        scroll_area::ScrollArea,
    },
    Component, Context, Theme,
};

#[derive(Serialize, Deserialize, Default, Clone)]
struct CommandRow {
    command: String,
}

struct ProcessState {
    child: Arc<Mutex<Option<Child>>>,
    output: Arc<Mutex<VecDeque<String>>>,
    needs_attention: Arc<Mutex<bool>>,
    is_running: Arc<Mutex<bool>>,
    is_suspended: Arc<Mutex<bool>>,
}

impl ProcessState {
    fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            output: Arc::new(Mutex::new(VecDeque::with_capacity(500))),
            needs_attention: Arc::new(Mutex::new(false)),
            is_running: Arc::new(Mutex::new(false)),
            is_suspended: Arc::new(Mutex::new(false)),
        }
    }
}

struct AppState {
    rows: Vec<CommandRow>,
    process_states: Vec<ProcessState>,
    focused_row: Option<usize>,
    cursor_pos: usize,
    scroll_offset: usize,
    output_scroll_offset: usize,
    interacting_row: Option<usize>,
    viewing_output_row: Option<usize>,
    should_quit: bool,
    theme: Theme,
    last_tick: Instant,
}

impl AppState {
    fn new() -> Self {
        let _ = fs::create_dir_all("output");
        let rows: Vec<CommandRow> = fs::read_to_string("commands.json")
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let process_states = (0..rows.len()).map(|_| ProcessState::new()).collect();

        Self {
            rows,
            process_states,
            focused_row: None,
            cursor_pos: 0,
            scroll_offset: 0,
            output_scroll_offset: 0,
            interacting_row: None,
            viewing_output_row: None,
            should_quit: false,
            theme: Theme::default(),
            last_tick: Instant::now(),
        }
    }

    fn save(&self) {
        let json = serde_json::to_string_pretty(&self.rows).unwrap();
        let _ = fs::write("commands.json", json);
    }

    fn add_row(&mut self) {
        self.rows.push(CommandRow { command: String::new() });
        self.process_states.push(ProcessState::new());
        self.save();
    }

    fn delete_row(&mut self, index: usize) {
        if index < self.rows.len() {
            self.kill_process(index);
            let _ = fs::remove_file(format!("output/cmd_{}.log", index));
            self.rows.remove(index);
            self.process_states.remove(index);
            if self.focused_row == Some(index) { self.focused_row = None; }
            else if let Some(ref mut fr) = self.focused_row { if *fr > index { *fr -= 1; } }
            self.save();
        }
    }

    fn exec_command(&mut self, index: usize) {
        let row = &self.rows[index];
        let state = &self.process_states[index];
        if *state.is_running.lock().unwrap() { return; }
        
        self.focused_row = None;
        let mut cmd_str = row.command.trim().to_string();
        if cmd_str.is_empty() { return; }

        if (cmd_str.starts_with("sudo ") || cmd_str == "sudo") && !cmd_str.contains(" -S") {
            if cmd_str == "sudo" { cmd_str = "sudo -S".to_string(); } 
            else { cmd_str = cmd_str.replacen("sudo ", "sudo -S ", 1); }
        }

        let log_path = format!("output/cmd_{}.log", index);
        let log_file = File::create(&log_path).ok();

        let child_res = Command::new("setsid")
            .arg("-w")
            .arg("sh")
            .arg("-c")
            .arg(cmd_str)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped())
            .spawn();

        if let Ok(mut child) = child_res {
            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();
            
            *state.child.lock().unwrap() = Some(child);
            *state.is_running.lock().unwrap() = true;
            *state.is_suspended.lock().unwrap() = false;
            *state.needs_attention.lock().unwrap() = false;
            state.output.lock().unwrap().clear();

            let output_clone = Arc::clone(&state.output);
            let attention_clone = Arc::clone(&state.needs_attention);
            let running_clone = Arc::clone(&state.is_running);
            let child_lock = Arc::clone(&state.child);
            let log_file_arc = Arc::new(Mutex::new(log_file));

            let log_file_stdout = Arc::clone(&log_file_arc);
            thread::spawn(move || {
                let mut reader = io::BufReader::new(stdout);
                let mut buffer = [0; 1024];
                while let Ok(n) = reader.get_mut().read(&mut buffer) {
                    if n == 0 { break; }
                    let s = String::from_utf8_lossy(&buffer[..n]);
                    if let Some(ref mut f) = *log_file_stdout.lock().unwrap() {
                        let _ = f.write_all(s.as_bytes());
                        let _ = f.flush();
                    }
                    let mut out = output_clone.lock().unwrap();
                    for part in s.split_inclusive('\n') {
                        if let Some(last) = out.back_mut() {
                            if !last.ends_with('\n') { last.push_str(part); }
                            else { if out.len() >= 500 { out.pop_front(); } out.push_back(part.to_string()); }
                        } else { out.push_back(part.to_string()); }
                    }
                    for line in out.iter().rev().take(3) {
                        let low = line.to_lowercase();
                        if low.contains("password") || low.contains("sudo") {
                            *attention_clone.lock().unwrap() = true;
                            break;
                        }
                    }
                }
            });

            let log_file_stderr = Arc::clone(&log_file_arc);
            let output_clone_err = Arc::clone(&state.output);
            let attention_clone_err = Arc::clone(&state.needs_attention);
            thread::spawn(move || {
                let mut reader = io::BufReader::new(stderr);
                let mut buffer = [0; 1024];
                while let Ok(n) = reader.get_mut().read(&mut buffer) {
                    if n == 0 { break; }
                    let s = String::from_utf8_lossy(&buffer[..n]);
                    if let Some(ref mut f) = *log_file_stderr.lock().unwrap() {
                        let _ = f.write_all(s.as_bytes());
                        let _ = f.flush();
                    }
                    let mut out = output_clone_err.lock().unwrap();
                    for part in s.split_inclusive('\n') {
                        if let Some(last) = out.back_mut() {
                            if !last.ends_with('\n') { last.push_str(part); }
                            else { if out.len() >= 500 { out.pop_front(); } out.push_back(part.to_string()); }
                        } else { out.push_back(part.to_string()); }
                    }
                    for line in out.iter().rev().take(3) {
                        let low = line.to_lowercase();
                        if low.contains("password") || low.contains("sudo") {
                            *attention_clone_err.lock().unwrap() = true;
                            break;
                        }
                    }
                }
            });

            thread::spawn(move || {
                loop {
                    {
                        let mut lock = child_lock.lock().unwrap();
                        if let Some(ref mut c) = *lock {
                            if let Ok(Some(_)) = c.try_wait() {
                                *running_clone.lock().unwrap() = false;
                                break;
                            }
                        } else { break; }
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            });
        }
    }

    fn toggle_suspend(&self, index: usize) {
        let state = &self.process_states[index];
        let mut child_lock = state.child.lock().unwrap();
        if let Some(ref mut child) = *child_lock {
            let pid = child.id();
            let mut suspended = state.is_suspended.lock().unwrap();
            if *suspended { let _ = Command::new("kill").arg("-CONT").arg(pid.to_string()).status(); *suspended = false; } 
            else { let _ = Command::new("kill").arg("-STOP").arg(pid.to_string()).status(); *suspended = true; }
        }
    }

    fn kill_process(&self, index: usize) {
        let state = &self.process_states[index];
        let mut child_lock = state.child.lock().unwrap();
        if let Some(ref mut child) = *child_lock {
            let _ = child.kill();
            *state.is_running.lock().unwrap() = false;
            *state.is_suspended.lock().unwrap() = false;
        }
    }

    fn sigint_process(&self, index: usize) {
        let state = &self.process_states[index];
        let mut child_lock = state.child.lock().unwrap();
        if let Some(ref mut child) = *child_lock {
            let pid = child.id();
            let _ = Command::new("kill").arg("-INT").arg(pid.to_string()).status();
        }
    }

    fn send_raw_input(&self, index: usize, data: &[u8]) {
        let state = &self.process_states[index];
        let mut lock = state.child.lock().unwrap();
        if let Some(ref mut child) = *lock {
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(data);
                let _ = stdin.flush();
            }
        }
    }
}

#[derive(Clone, Debug)]
enum AppAction {
    AddRow,
    FocusInput(usize),
    ExecCommand(usize),
    Interact(usize),
    Suspend(usize),
    Kill(usize),
    DeleteRow(usize),
    ViewOutput(usize),
}

struct ButtonListRenderer {
    scroll_offset: usize,
    focused_row: Option<usize>,
    cursor_pos: usize,
    last_tick: Instant,
}

impl Component<AppState, AppAction> for ButtonListRenderer {
    fn render(&self, f: &mut Frame, state: &mut AppState, area: Rect, context: &mut Context<AppAction>) {
        let row_height = 3;
        let visible_rows = (area.height as usize) / row_height;
        let start_idx = self.scroll_offset;
        let end_idx = (start_idx + visible_rows).min(state.rows.len());

        let constraints = (start_idx..end_idx).map(|_| Constraint::Length(row_height as u16)).collect::<Vec<_>>();
        let chunks = Layout::default().direction(Direction::Vertical).constraints(constraints).split(area);

        for (i, row_idx) in (start_idx..end_idx).enumerate() {
            let row = &state.rows[row_idx];
            let row_chunk = chunks[i];
            let proc_state = &state.process_states[row_idx];
            let is_running = *proc_state.is_running.lock().unwrap();
            let is_suspended = *proc_state.is_suspended.lock().unwrap();
            let needs_attention = *proc_state.needs_attention.lock().unwrap();
            let log_exists = fs::metadata(format!("output/cmd_{}.log", row_idx)).is_ok();

            let row_inner = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Length(7), Constraint::Min(0)]).split(row_chunk);

            let exec_btn = Button::new("EXEC", AppAction::ExecCommand(row_idx)).style(ButtonStyle::Plain).disabled(is_running);
            exec_btn.render(f, &mut (), row_inner[0], context);

            let input = TextInput::new("", row.command.clone(), AppAction::FocusInput(row_idx))
                .focused(self.focused_row == Some(row_idx))
                .disabled(is_running)
                .cursor_pos(if self.focused_row == Some(row_idx) { self.cursor_pos } else { row.command.len() })
                .placeholder("Enter command...");
            input.render(f, &mut (), row_inner[1], context);

            let top_y = row_inner[1].y;
            if is_running {
                let x_start = row_inner[1].x + 2;
                let is_flash_on = (self.last_tick.elapsed().as_millis() / 500) % 2 == 0;
                let (focus_label, focus_style) = if needs_attention {
                    if is_flash_on { ("! FOCUS !", Style::default().fg(Color::Red)) } 
                    else { ("[ FOCUS ]", Style::default().fg(Color::Indexed(8))) }
                } else { ("[ FOCUS ]", Style::default().fg(Color::Yellow)) };

                let focus_rect = Rect::new(x_start, top_y, focus_label.len() as u16, 1);
                TextButton::new(focus_label, AppAction::Interact(row_idx)).style(focus_style).render(f, &mut (), focus_rect, context);

                let suspend_label = if is_suspended { "[ RESUME ]" } else { "[ SUSPEND ]" };
                let suspend_rect = Rect::new(x_start + focus_label.len() as u16 + 2, top_y, suspend_label.len() as u16, 1);
                TextButton::new(suspend_label, AppAction::Suspend(row_idx)).style(Style::default().fg(Color::Cyan)).render(f, &mut (), suspend_rect, context);

                let kill_label = "[ KILL ]";
                let kill_rect = Rect::new(row_inner[1].x + row_inner[1].width - kill_label.len() as u16 - 8, top_y, kill_label.len() as u16, 1);
                TextButton::new(kill_label, AppAction::Kill(row_idx)).style(Style::default().fg(Color::Red)).render(f, &mut (), kill_rect, context);
            } else if log_exists {
                let x_start = row_inner[1].x + 2;
                let view_label = "[ VIEW OUT ]";
                let view_rect = Rect::new(x_start, top_y, view_label.len() as u16, 1);
                TextButton::new(view_label, AppAction::ViewOutput(row_idx)).style(Style::default().fg(Color::Green)).render(f, &mut (), view_rect, context);
            }

            let delete_label = "[ X ]";
            let delete_rect = Rect::new(row_inner[1].x + row_inner[1].width - delete_label.len() as u16 - 2, top_y, delete_label.len() as u16, 1);
            TextButton::new(delete_label, AppAction::DeleteRow(row_idx)).style(Style::default().fg(Color::DarkGray)).render(f, &mut (), delete_rect, context);
        }
    }
}

struct LogRenderer {
    lines: Vec<String>,
    scroll_offset: usize,
}

impl Component<(), AppAction> for LogRenderer {
    fn render(&self, f: &mut Frame, _state: &mut (), area: Rect, _context: &mut Context<AppAction>) {
        let visible_lines = area.height as usize;
        let start = self.scroll_offset;
        let end = (start + visible_lines).min(self.lines.len());
        let items: Vec<ListItem> = self.lines[start..end].iter().map(|s| ListItem::new(s.clone())).collect();
        f.render_widget(List::new(items), area);
    }
}

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, event::EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = AppState::new();

    loop {
        if state.should_quit { break; }

        let size = terminal.get_frame().area();
        let chunks = Layout::default().direction(Direction::Vertical).margin(1).constraints([Constraint::Length(1), Constraint::Min(0)]).split(size);
        let visible_rows = (chunks[1].height.saturating_sub(2) as usize) / 3;
        state.scroll_offset = state.scroll_offset.min(state.rows.len().saturating_sub(visible_rows));

        let mut clickable_areas = Vec::new();

        terminal.draw(|f| {
            let mut context = Context::new(&mut clickable_areas, state.theme);
            let f_area = f.area();

            if let Some(idx) = state.viewing_output_row {
                let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(0), Constraint::Length(1)]).split(f_area);
                let content = fs::read_to_string(format!("output/cmd_{}.log", idx)).unwrap_or_else(|_| "Error reading log".to_string());
                let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
                let scroll_area = ScrollArea::new(LogRenderer { lines: lines.clone(), scroll_offset: state.output_scroll_offset }, state.output_scroll_offset, lines.len(), chunks[0].height as usize);
                scroll_area.title(format!(" Output: {} ", state.rows[idx].command)).render(f, &mut (), chunks[0], &mut context);
                f.render_widget(Paragraph::new(" ESC: Close | Scroll/PgUp/PgDn to view more ").style(Style::default().fg(Color::DarkGray)), chunks[1]);
                return;
            }

            if let Some(idx) = state.interacting_row {
                let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(0), Constraint::Length(1)]).split(f_area);
                let proc_state = &state.process_states[idx];
                let output = proc_state.output.lock().unwrap();
                
                let visible_height = chunks[0].height.saturating_sub(2) as usize; // inside borders
                let total_lines = output.len();
                let start_idx = total_lines.saturating_sub(visible_height);
                let visible_lines: Vec<ListItem> = output.iter().skip(start_idx).map(|s| ListItem::new(s.clone())).collect();
                
                let list = List::new(visible_lines)
                    .block(Block::default().borders(Borders::ALL).title(format!(" Live Interaction: {} ", state.rows[idx].command)));
                
                f.render_widget(list, chunks[0]);
                f.render_widget(Paragraph::new(" ESC: Back | CTRL+C: SIGINT | Keys are sent directly to process ").style(Style::default().fg(Color::Cyan)), chunks[1]);
                return;
            }

            let chunks = Layout::default().direction(Direction::Vertical).margin(1).constraints([Constraint::Length(1), Constraint::Min(0)]).split(f_area);
            TextButton::new(" (Add Button Command)", AppAction::AddRow).render(f, &mut (), chunks[0], &mut context);
            let scroll_area = ScrollArea::new(ButtonListRenderer { scroll_offset: state.scroll_offset, focused_row: state.focused_row, cursor_pos: state.cursor_pos, last_tick: state.last_tick }, state.scroll_offset, state.rows.len(), visible_rows);
            scroll_area.render(f, &mut state, chunks[1], &mut context);
        })?;

        if let Some(idx) = state.interacting_row {
            if !*state.process_states[idx].is_running.lock().unwrap() { state.interacting_row = None; }
        }

        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) => {
                    if let Some(idx) = state.viewing_output_row {
                        match key.code {
                            KeyCode::Esc => { state.viewing_output_row = None; state.output_scroll_offset = 0; }
                            KeyCode::Up => { if state.output_scroll_offset > 0 { state.output_scroll_offset -= 1; } }
                            KeyCode::Down => { state.output_scroll_offset += 1; }
                            KeyCode::PageUp => { state.output_scroll_offset = state.output_scroll_offset.saturating_sub(10); }
                            KeyCode::PageDown => { state.output_scroll_offset += 10; }
                            _ => {}
                        }
                    } else if let Some(idx) = state.interacting_row {
                        match key.code {
                            KeyCode::Esc => { state.interacting_row = None; }
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => { state.sigint_process(idx); }
                            KeyCode::Char(c) => { state.send_raw_input(idx, c.to_string().as_bytes()); }
                            KeyCode::Enter => { 
                                state.send_raw_input(idx, b"\n"); 
                                { let mut out = state.process_states[idx].output.lock().unwrap(); if let Some(last) = out.back_mut() { if !last.ends_with('\n') { last.push('\n'); } } }
                                *state.process_states[idx].needs_attention.lock().unwrap() = false; 
                            }
                            KeyCode::Backspace => { state.send_raw_input(idx, b"\x08"); }
                            KeyCode::Tab => { state.send_raw_input(idx, b"\t"); }
                            _ => {}
                        }
                    } else if let Some(i) = state.focused_row {
                        match key.code {
                            KeyCode::Enter | KeyCode::Esc => { state.focused_row = None; state.save(); }
                            KeyCode::Left => { if state.cursor_pos > 0 { state.cursor_pos -= 1; } }
                            KeyCode::Right => { if state.cursor_pos < state.rows[i].command.len() { state.cursor_pos += 1; } }
                            KeyCode::Char(c) => { state.rows[i].command.insert(state.cursor_pos, c); state.cursor_pos += 1; }
                            KeyCode::Backspace => { if state.cursor_pos > 0 { state.rows[i].command.remove(state.cursor_pos - 1); state.cursor_pos -= 1; } }
                            KeyCode::Delete => { if state.cursor_pos < state.rows[i].command.len() { state.rows[i].command.remove(state.cursor_pos); } }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('q') => state.should_quit = true,
                            KeyCode::Up => { if state.scroll_offset > 0 { state.scroll_offset -= 1; } }
                            KeyCode::Down => { let max = state.rows.len().saturating_sub(visible_rows); if state.scroll_offset < max { state.scroll_offset += 1; } }
                            KeyCode::PageUp => { state.scroll_offset = state.scroll_offset.saturating_sub(visible_rows); }
                            KeyCode::PageDown => { 
                                let max = state.rows.len().saturating_sub(visible_rows);
                                state.scroll_offset = (state.scroll_offset + visible_rows).min(max);
                            }
                            _ => {}
                        }
                    }
                }
                Event::Mouse(me) => {
                    match me.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            let context = Context::new(&mut clickable_areas, state.theme);
                            if let Some(ca) = context.handle_click_ca(me.column, me.row) {
                                match ca.action {
                                    AppAction::AddRow => state.add_row(),
                                    AppAction::FocusInput(i) => {
                                        if !*state.process_states[i].is_running.lock().unwrap() {
                                            state.focused_row = Some(i);
                                            state.cursor_pos = (me.column.saturating_sub(ca.area.x + 1) as usize).min(state.rows[i].command.len());
                                        }
                                    }
                                    AppAction::ExecCommand(i) => state.exec_command(i),
                                    AppAction::Interact(i) => { state.interacting_row = Some(i); state.focused_row = None; state.output_scroll_offset = 0; }
                                    AppAction::Suspend(i) => state.toggle_suspend(i),
                                    AppAction::Kill(i) => state.kill_process(i),
                                    AppAction::DeleteRow(i) => state.delete_row(i),
                                    AppAction::ViewOutput(i) => { state.viewing_output_row = Some(i); state.output_scroll_offset = 0; }
                                }
                            } else { state.focused_row = None; state.save(); }
                        }
                        MouseEventKind::ScrollUp => {
                            if state.viewing_output_row.is_some() {
                                if state.output_scroll_offset > 0 { state.output_scroll_offset -= 1; }
                            } else if state.interacting_row.is_none() {
                                if state.scroll_offset > 0 { state.scroll_offset -= 1; }
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            if state.viewing_output_row.is_some() {
                                state.output_scroll_offset += 1;
                            } else if state.interacting_row.is_none() {
                                let max = state.rows.len().saturating_sub(visible_rows);
                                if state.scroll_offset < max { state.scroll_offset += 1; }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, event::DisableMouseCapture)?;
    terminal.show_cursor()?;
    Ok(())
}
