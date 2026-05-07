use std::path::{Path, PathBuf};

use crate::error::Result;

#[derive(Debug, Clone, PartialEq)]
pub enum DiffKind {
    New,
    Modified,
    #[allow(dead_code)]
    Deleted,
}

#[derive(Debug, Clone)]
pub struct DiffItem {
    pub real_path: PathBuf,
    pub kind: DiffKind,
    pub size_bytes: u64,
    pub selected: bool,
}

/// Walk real paths and compare against snapshot to produce a diff list.
pub fn compute_diff(real_paths: &[PathBuf], snapshot_root: &Path) -> Result<Vec<DiffItem>> {
    let mut items = vec![];

    for real_path in real_paths {
        collect_diff_items(real_path, real_path, snapshot_root, &mut items)?;
    }

    Ok(items)
}

fn collect_diff_items(
    _base: &Path,
    current: &Path,
    snapshot_root: &Path,
    items: &mut Vec<DiffItem>,
) -> Result<()> {
    if current.is_dir() {
        for entry in std::fs::read_dir(current)? {
            let entry = entry?;
            collect_diff_items(_base, &entry.path(), snapshot_root, items)?;
        }
    } else {
        let rel = current.strip_prefix("/").unwrap_or(current);
        let snap_path = snapshot_root.join(rel);

        let kind = if !snap_path.exists() {
            DiffKind::New
        } else {
            let real_content = std::fs::read(current)?;
            let snap_content = std::fs::read(&snap_path)?;
            if real_content != snap_content {
                DiffKind::Modified
            } else {
                return Ok(());
            }
        };

        let size_bytes = current.metadata().map(|m| m.len()).unwrap_or(0);
        items.push(DiffItem {
            real_path: current.to_path_buf(),
            kind,
            size_bytes,
            selected: true,
        });
    }

    Ok(())
}

/// Show the review TUI. Returns the list of real paths the user chose to keep.
pub fn show_review_tui(exit_code: i32, mut items: Vec<DiffItem>) -> Result<Vec<PathBuf>> {
    if items.is_empty() {
        return Ok(vec![]);
    }

    use crossterm::{
        event::{self, Event, KeyCode},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    };
    use ratatui::{
        Terminal,
        backend::CrosstermBackend,
        layout::{Constraint, Direction, Layout},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    };

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut list_state = ListState::default();
    list_state.select(Some(0));

    let result = loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(3)])
                .split(f.area());

            let list_items: Vec<ListItem> = items
                .iter()
                .map(|item| {
                    let check = if item.selected { "[x]" } else { "[ ]" };
                    let kind = match item.kind {
                        DiffKind::New => "+",
                        DiffKind::Modified => "~",
                        DiffKind::Deleted => "-",
                    };
                    let size = format!("{} B", item.size_bytes);
                    let line = Line::from(vec![
                        Span::raw(format!("  {check} {kind} ")),
                        Span::styled(
                            item.real_path.display().to_string(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(format!("  {size}")),
                    ]);
                    ListItem::new(line)
                })
                .collect();

            let title = format!(" inbox: review changes (command exited {exit_code}) ");
            let list = List::new(list_items)
                .block(Block::default().borders(Borders::ALL).title(title))
                .highlight_style(Style::default().bg(Color::DarkGray));

            f.render_stateful_widget(list, chunks[0], &mut list_state);

            let help =
                Paragraph::new("space: toggle  a: all  n: none  enter: apply  q: discard all")
                    .block(Block::default().borders(Borders::ALL));
            f.render_widget(help, chunks[1]);
        })?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') => break vec![],
                KeyCode::Enter => {
                    break items
                        .iter()
                        .filter(|i| i.selected)
                        .map(|i| i.real_path.clone())
                        .collect();
                }
                KeyCode::Char(' ') => {
                    if let Some(i) = list_state.selected() {
                        items[i].selected = !items[i].selected;
                    }
                }
                KeyCode::Char('a') => items.iter_mut().for_each(|i| i.selected = true),
                KeyCode::Char('n') => items.iter_mut().for_each(|i| i.selected = false),
                KeyCode::Down | KeyCode::Char('j') => {
                    let next = list_state
                        .selected()
                        .map(|i| (i + 1).min(items.len() - 1))
                        .unwrap_or(0);
                    list_state.select(Some(next));
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    let prev = list_state
                        .selected()
                        .map(|i| i.saturating_sub(1))
                        .unwrap_or(0);
                    list_state.select(Some(prev));
                }
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn compute_diff_detects_modified() {
        let real_dir = TempDir::new().unwrap();
        let snap_dir = TempDir::new().unwrap();
        let real_file = real_dir.path().join("f.txt");
        fs::write(&real_file, "modified").unwrap();

        // Mirror the path structure: snap_root/strip_leading_slash(real_file)
        let rel = real_file.strip_prefix("/").unwrap_or(&real_file);
        let snap_file = snap_dir.path().join(rel);
        fs::create_dir_all(snap_file.parent().unwrap()).unwrap();
        fs::write(&snap_file, "original").unwrap();

        let diff = compute_diff(&[real_dir.path().to_path_buf()], snap_dir.path()).unwrap();
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].kind, DiffKind::Modified);
    }

    #[test]
    fn compute_diff_detects_new_file() {
        let real_dir = TempDir::new().unwrap();
        let snap_dir = TempDir::new().unwrap();
        fs::write(real_dir.path().join("new.txt"), "new").unwrap();
        // snap_dir is empty — no corresponding file in snapshot

        let diff = compute_diff(&[real_dir.path().to_path_buf()], snap_dir.path()).unwrap();
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].kind, DiffKind::New);
    }

    #[test]
    fn compute_diff_ignores_unchanged() {
        let real_dir = TempDir::new().unwrap();
        let snap_dir = TempDir::new().unwrap();
        let real_file = real_dir.path().join("same.txt");
        fs::write(&real_file, "same").unwrap();

        // Mirror the path structure
        let rel = real_file.strip_prefix("/").unwrap_or(&real_file);
        let snap_file = snap_dir.path().join(rel);
        fs::create_dir_all(snap_file.parent().unwrap()).unwrap();
        fs::write(&snap_file, "same").unwrap();

        let diff = compute_diff(&[real_dir.path().to_path_buf()], snap_dir.path()).unwrap();
        assert!(diff.is_empty());
    }
}
