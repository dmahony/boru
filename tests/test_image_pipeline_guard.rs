//! BORU-IFS-22 Required Test 17: protect the established image-send pipeline.
//!
//! This is intentionally a source-level regression test. `ExecuteImageSend`
//! lives in the GUI example and is not independently executable without starting
//! the application, but its ordering is the compatibility contract: image bytes
//! are read, GIFs are preserved or other images are optimized to WebP, and only
//! then is `ImageShare` announced. Images must not be routed through `FileOffer`.

const FILES_RS: &str = include_str!("../src/bin/boru/app/files.rs");

fn image_send_arm() -> &'static str {
    let start = FILES_RS
        .find("AppMessage::ExecuteImageSend(encoded)")
        .expect("ExecuteImageSend arm must remain present");
    let end = FILES_RS[start..]
        .find("AppMessage::ExecuteDownload")
        .map(|offset| start + offset)
        .expect("ExecuteImageSend arm must terminate before downloads");
    &FILES_RS[start..end]
}

#[test]
fn required_test_17_image_sharing_remains_on_its_existing_pipeline() {
    let arm = image_send_arm();

    let read = arm
        .find("tokio::fs::read(&path_buf)")
        .expect("image sending must read image bytes before processing");
    let optimization = arm
        .find("optimize_chat_image_to_webp")
        .expect("non-GIF images must retain WebP optimization");
    let announcement = arm
        .find("crate::Message::ImageShare")
        .expect("image sending must announce ImageShare");

    assert!(
        read < optimization && optimization < announcement,
        "image bytes must be read and optimized before ImageShare is announced"
    );
    assert!(
        arm.contains("Transmit GIF bytes unchanged")
            && arm.contains("let is_gif = filename.to_lowercase().ends_with(\".gif\")"),
        "animated GIFs must retain the unchanged-byte path"
    );
    assert!(
        !arm.contains("FileOffer"),
        "ExecuteImageSend must not be routed through the generic FileOffer path"
    );
}

#[test]
fn generic_file_send_owns_the_file_offer_path() {
    let file_start = FILES_RS
        .find("AppMessage::ExecuteFileSend(encoded)")
        .expect("ExecuteFileSend arm must remain present");
    let image_start = FILES_RS
        .find("AppMessage::ExecuteImageSend(encoded)")
        .expect("ExecuteImageSend arm must remain present");
    let file_arm = &FILES_RS[file_start..image_start];

    assert!(
        file_arm.contains("FileOffer"),
        "generic ExecuteFileSend must contain the direct FileOffer path"
    );
}
