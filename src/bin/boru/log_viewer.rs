#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::large_enum_variant,
    clippy::if_same_then_else,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    clippy::redundant_guards,
    clippy::manual_let_else,
    clippy::vec_init_then_push,
    clippy::let_underscore_future,
    clippy::needless_update,
    clippy::unnecessary_unwrap,
    clippy::single_match,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::question_mark,
    clippy::unnecessary_sort_by,
    clippy::result_large_err,
    clippy::enum_variant_names,
    clippy::explicit_counter_loop,
    clippy::wrong_self_convention,
    missing_debug_implementations,
    unfulfilled_lint_expectations
)]
#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use iced::{
    widget::{button, column, container, row},
    Element, Length,
};
use n0_error::{Result as NResult, StdResultExt};

use crate::app;
use crate::app::text_muted_style;

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
        use crate::app::SPACE_12;

        let header = row![
            crate::fonts::type_role_text(crate::fonts::TypeRole::SectionTitle, "Boru logs"),
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                format!(" {}", app::version_tag()),
            )
            .style(text_muted_style)
        ]
        .spacing(SPACE_12)
        .push(
            button(crate::fonts::type_role_text(
                crate::fonts::TypeRole::ButtonLabel,
                "Reload",
            ))
            .on_press(Message::Refresh),
        );

        let body = if self.contents.is_empty() {
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                format!(
                    "No log output yet.\n\nThe log file is:\n{}",
                    self.log_path.display()
                ),
            )
        } else {
            crate::fonts::type_role_text(crate::fonts::TypeRole::TechnicalValue, &self.contents)
                .size(14.0)
                .width(Length::Fill)
        };

        column![
            header,
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                self.log_path.display().to_string(),
            )
            .style(text_muted_style),
            crate::ui_components::gutter_scrollable(container(body).width(Length::Fill))
                .height(Length::Fill),
        ]
        .spacing(SPACE_12)
        .padding(SPACE_12)
        .into()
    }
}

pub fn log_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("logs").join("boru.log")
}

#[expect(dead_code)]
pub fn spawn(data_dir: &Path) -> std::result::Result<(), String> {
    build_spawn_command(data_dir)?
        .spawn()
        .map_err(|e| format!("failed to launch log viewer: {e}"))?;
    Ok(())
}

#[expect(dead_code)]
fn build_spawn_command(data_dir: &Path) -> std::result::Result<Command, String> {
    let exe =
        std::env::current_exe().map_err(|e| format!("failed to locate current executable: {e}"))?;
    let mut cmd = Command::new(exe);
    cmd.arg("logs")
        .env("BORU_DATA_DIR", data_dir)
        .env("BORU_CHAT_DATA_DIR", data_dir);
    Ok(cmd)
}

pub fn run(log_path: PathBuf) -> NResult<()> {
    let state = LogViewer::load(log_path.clone());
    iced::application(move || (state.clone(), iced::Task::none()), update, view)
        .title(move |_: &LogViewer| {
            format!("Boru logs {} — {}", app::version_tag(), log_path.display())
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

        // Check that BORU_DATA_DIR is set (new)
        assert!(
            cmd.get_envs()
                .any(|(key, _)| key == OsStr::new("BORU_DATA_DIR")),
            "BORU_DATA_DIR should be set"
        );
        let new_env = cmd
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("BORU_DATA_DIR"))
            .and_then(|(_, value)| value)
            .expect("BORU_DATA_DIR env should be set");
        assert_eq!(new_env, data_dir.as_os_str());

        // Check that BORU_CHAT_DATA_DIR is set (legacy)
        let legacy_env = cmd
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("BORU_CHAT_DATA_DIR"))
            .and_then(|(_, value)| value)
            .expect("BORU_CHAT_DATA_DIR env should be set");
        assert_eq!(legacy_env, data_dir.as_os_str());
        assert!(!args.iter().any(|arg| arg == "--data-dir"));
    }
}
