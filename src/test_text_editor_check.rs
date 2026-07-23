use iced::widget::text_editor;
fn main() {
    let content = text_editor::Content::new();
    let _editor = text_editor::TextEditor::new(&content);
}
