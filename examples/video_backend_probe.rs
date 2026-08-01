//! Developer-only probe for the isolated Iced/GStreamer video backend.
//!
//! Run with:
//! `cargo run --example video_backend_probe --features video-playback -- path/to/video`
//!
//! This intentionally does not know about chat messages or attachment state.

use iced::widget::{button, column, row, slider, text, Container};
use iced::{Element, Length, Subscription, Task};
use iced_video_player::{Video, VideoPlayer};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
enum BackendEvent {
    Loaded { video: Arc<Video>, path: PathBuf },
    LoadingFailed(String),
    FramePresented,
    EndOfStream,
    Error(String),
    Resized { width: u32, height: u32 },
}

#[derive(Debug, Clone)]
enum Message {
    Backend(BackendEvent),
    TogglePause,
    Seek(f64),
    SeekRelease,
    SeekBy(f64),
    ToggleMute,
    Resize(iced::Size),
}

struct App {
    video: Option<Arc<Video>>,
    path: Option<PathBuf>,
    position: f64,
    dragging: bool,
    status: String,
    last_event: String,
    viewport: iced::Size,
}

impl Default for App {
    fn default() -> Self {
        Self {
            path: std::env::args().nth(1).map(PathBuf::from),
            video: None,
            position: 0.0,
            dragging: false,
            status: "Loading local file…".into(),
            last_event: "startup".into(),
            viewport: iced::Size::ZERO,
        }
    }
}

fn load_video(path: PathBuf) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let canonical = path
                    .canonicalize()
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                let uri = url::Url::from_file_path(&canonical)
                    .map_err(|()| format!("cannot make a file URI for {}", canonical.display()))?;
                Video::new(&uri)
                    .map(|video| (video, canonical))
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result)
        },
        |result| match result {
            Ok((video, path)) => Message::Backend(BackendEvent::Loaded {
                video: Arc::new(video),
                path,
            }),
            Err(error) => Message::Backend(BackendEvent::LoadingFailed(error)),
        },
    )
}

impl App {
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Backend(event) => {
                self.last_event = format!("{event:?}");
                match event {
                    BackendEvent::Loaded { video, path } => {
                        self.video = Some(video);
                        self.path = Some(path);
                        self.status = "Backend loaded; press Play".into();
                    }
                    BackendEvent::LoadingFailed(error) => {
                        self.status = format!("Load error: {error}")
                    }
                    BackendEvent::FramePresented => {
                        self.status = "Playing".into();
                        if !self.dragging {
                            if let Some(video) = &self.video {
                                self.position = video.position().as_secs_f64();
                            }
                        }
                    }
                    BackendEvent::EndOfStream => self.status = "End of stream".into(),
                    BackendEvent::Error(error) => self.status = format!("Playback error: {error}"),
                    BackendEvent::Resized { width, height } => {
                        self.status = format!("Resized to {width}×{height}");
                    }
                }
            }
            Message::TogglePause => {
                if let Some(video) = self.video.as_mut().and_then(Arc::get_mut) {
                    let paused = !video.paused();
                    video.set_paused(paused);
                    self.status = if paused { "Paused" } else { "Playing" }.into();
                }
            }
            Message::Seek(position) => {
                self.dragging = true;
                self.position = position;
                if let Some(video) = self.video.as_mut().and_then(Arc::get_mut) {
                    video.set_paused(true);
                }
            }
            Message::SeekRelease => {
                self.dragging = false;
                if let Some(video) = self.video.as_mut().and_then(Arc::get_mut) {
                    if let Err(error) = video.seek(Duration::from_secs_f64(self.position), false) {
                        return Task::done(Message::Backend(BackendEvent::Error(
                            error.to_string(),
                        )));
                    }
                    video.set_paused(false);
                }
            }
            Message::SeekBy(delta) => {
                let duration = self
                    .video
                    .as_ref()
                    .map_or(0.0, |video| video.duration().as_secs_f64());
                self.position = (self.position + delta).clamp(0.0, duration);
                if let Some(video) = self.video.as_mut().and_then(Arc::get_mut) {
                    if let Err(error) = video.seek(Duration::from_secs_f64(self.position), false) {
                        return Task::done(Message::Backend(BackendEvent::Error(
                            error.to_string(),
                        )));
                    }
                }
            }
            Message::ToggleMute => {
                if let Some(video) = self.video.as_mut().and_then(Arc::get_mut) {
                    let muted = !video.muted();
                    video.set_muted(muted);
                }
            }
            Message::Resize(size) => {
                self.viewport = size;
                return Task::done(Message::Backend(BackendEvent::Resized {
                    width: size.width as u32,
                    height: size.height as u32,
                }));
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let video = self.video.as_ref().map(|video| {
            Container::new(
                VideoPlayer::new(video)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(iced::ContentFit::Contain)
                    .on_end_of_stream(Message::Backend(BackendEvent::EndOfStream))
                    .on_new_frame(Message::Backend(BackendEvent::FramePresented))
                    .on_error(|error| Message::Backend(BackendEvent::Error(error.to_string()))),
            )
            .width(Length::Fill)
            .height(Length::Fill)
        });
        let body: Element<'_, Message> = video.map_or_else(
            || text("No player loaded (pass a local video path as the first argument)").into(),
            |video| video.into(),
        );
        let duration = self
            .video
            .as_ref()
            .map_or(0.0, |video| video.duration().as_secs_f64());
        column![
            body,
            slider(0.0..=duration.max(0.1), self.position, Message::Seek)
                .on_release(Message::SeekRelease),
            row![
                button(text(
                    if self.video.as_ref().is_some_and(|video| video.paused()) {
                        "Play"
                    } else {
                        "Pause"
                    }
                ))
                .on_press(Message::TogglePause),
                button("−5s").on_press(Message::SeekBy(-5.0)),
                button("+5s").on_press(Message::SeekBy(5.0)),
                button("Mute").on_press(Message::ToggleMute),
            ],
            text(format!(
                "{} | {} | viewport {:.0}×{:.0}",
                self.status, self.last_event, self.viewport.width, self.viewport.height
            )),
        ]
        .spacing(8)
        .padding(12)
        .into()
    }
}

fn subscription(_app: &App) -> Subscription<Message> {
    iced::event::listen().filter_map(|event| match event {
        iced::Event::Window(iced::window::Event::Resized(size)) => Some(Message::Resize(size)),
        _ => None,
    })
}

fn main() -> iced::Result {
    iced::application(
        || {
            let app = App::default();
            let task = app.path.clone().map_or_else(Task::none, load_video);
            (app, task)
        },
        App::update,
        App::view,
    )
    .subscription(subscription)
    .run()
}
