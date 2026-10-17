use iced::widget::{column, text, text_editor};
use iced::Element;

fn main() -> iced::Result {
    iced::application(App::default, App::update, App::view).run()
}

#[derive(Default)]
struct App {
    content: text_editor::Content,
}

#[derive(Debug, Clone)]
enum Message {
    Edit(text_editor::Action),
}

impl App {
    fn update(&mut self, message: Message) {
        match message {
            Message::Edit(action) => self.content.perform(action),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        column![text_editor(&self.content).on_action(Message::Edit), text("hi")].into()
    }
}
