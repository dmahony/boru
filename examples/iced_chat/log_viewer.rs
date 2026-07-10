use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use iced::{widget::{button, column, container, row, scrollable, text}, Element, Length};
use n0_error::{Result as NResult, StdResultExt};

#[derive(Debug, Clone)]
pub enum Message {
    Refresh,
}

#[derive(Debug, Clone)]
pub struct LogViewer {
    log_path: PathBuf,
    contents: String,
}

impl LogViewer {
    fn load(log_path: PathBuf) -> Self {
        let contents = read_log(&log_path);
        Self { log_path, contents }
    }

    fn reload(&mut self) {
        self.contents = read_log(&self.log_path);
    }

    fn view(&self) -> Element<'_, Message> {
        let header = row![text("Iroh Gossip Chat logs").size(22)]
            .spacing(12)
            .push(button("Reload").on_press(Message::Refresh));

        let body = if self.contents.is_empty() {
            text(format!(
                "No log output yet.\n\nThe log file is:\n{}",
                self.log_path.display()
            ))
            .size(14)
        } else {
            text(&self.contents)
                .font(iced::Font::MONOSPACE)
                .size(13)
                .width(Length::Fill)
        };

        column![
            header,
            text(self.log_path.display().to_string()).size(12),
            scrollable(container(body).width(Length::Fill)).height(Length::Fill),
        ]
        .spacing(12)
        .padding(12)
        .into()
    }
}

pub fn log_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("logs").join("iced_chat.log")
}

pub fn spawn(data_dir: &Path) -> std::result::Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("failed to locate current executable: {e}"))?;
    Command::new(exe)
        .arg("logs")
        .arg("--data-dir")
        .arg(data_dir)
        .spawn()
        .map_err(|e| format!("failed to launch log viewer: {e}"))?;
    Ok(())
}

pub fn run(log_path: PathBuf) -> NResult<()> {
    let state = LogViewer::load(log_path.clone());
    iced::application(move || (state.clone(), iced::Task::none()), update, view)
        .title(move |_: &LogViewer| format!("Iroh Gossip Chat logs — {}", log_path.display()))
        .subscription(|_| iced::time::every(Duration::from_secs(1)).map(|_| Message::Refresh))
        .run()
        .std_context("failed to run log viewer")?;
    Ok(())
}

fn update(state: &mut LogViewer, message: Message) -> iced::Task<Message> {
    match message {
        Message::Refresh => {
            state.reload();
            iced::Task::none()
        }
    }
}

fn view(state: &LogViewer) -> Element<'_, Message> {
    state.view()
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|_| String::new())
}
