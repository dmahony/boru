use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use iced::{
    widget::{button, column, container, row, scrollable, text},
    Element, Length,
};
use n0_error::{Result as NResult, StdResultExt};

use crate::app;
use crate::app::{text_muted_style, TYPO_XXS};

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
        let header = row![
            text("Boru Chat logs").size(22),
            text(format!(" {}", app::version_tag()))
                .size(TYPO_XXS)
                .style(text_muted_style)
        ]
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
    build_spawn_command(data_dir)?
        .spawn()
        .map_err(|e| format!("failed to launch log viewer: {e}"))?;
    Ok(())
}

fn build_spawn_command(data_dir: &Path) -> std::result::Result<Command, String> {
    let exe =
        std::env::current_exe().map_err(|e| format!("failed to locate current executable: {e}"))?;
    let mut cmd = Command::new(exe);
    cmd.arg("logs").env("BORU_CHAT_DATA_DIR", data_dir);
    Ok(cmd)
}

pub fn run(log_path: PathBuf) -> NResult<()> {
    let state = LogViewer::load(log_path.clone());
    iced::application(move || (state.clone(), iced::Task::none()), update, view)
        .title(move |_: &LogViewer| {
            format!(
                "Boru Chat logs {} — {}",
                app::version_tag(),
                log_path.display()
            )
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn build_spawn_command_sets_data_dir_env_and_keeps_logs_as_the_only_argument() {
        let data_dir = Path::new("/tmp/boru-chat");
        let cmd = build_spawn_command(data_dir).expect("command should build");

        let args: Vec<_> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["logs"]);

        let env_value = cmd
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("BORU_CHAT_DATA_DIR"))
            .and_then(|(_, value)| value)
            .expect("data dir env should be set");
        assert_eq!(env_value, data_dir.as_os_str());
        assert!(!args.iter().any(|arg| arg == "--data-dir"));
    }
}
