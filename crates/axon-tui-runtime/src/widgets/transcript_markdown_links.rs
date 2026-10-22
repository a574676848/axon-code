//! Transcript Markdown 链接上下文。

use crate::widgets::{
    terminal_hyperlinks::web_destination, terminal_local_links::display_path_for_link,
};

#[derive(Debug, Clone)]
pub(super) struct LinkContext {
    destination: String,
    original: String,
    display: Option<String>,
    show_destination: bool,
}

impl LinkContext {
    pub(super) fn from_destination(destination: &str) -> Self {
        if let Some(destination_url) = web_destination(destination) {
            return Self {
                destination: destination_url,
                original: destination.to_owned(),
                display: None,
                show_destination: true,
            };
        }
        let (display, destination_url) = display_path_for_link(destination).unwrap_or_else(|| {
            (
                String::new(),
                destination.chars().filter(|ch| !ch.is_control()).collect(),
            )
        });
        let is_local_target = !display.is_empty();
        Self {
            destination: destination_url,
            original: destination.to_owned(),
            display: is_local_target.then_some(display),
            show_destination: !is_local_target,
        }
    }

    pub(super) fn destination(&self) -> String {
        self.destination.clone()
    }

    pub(super) fn display_text_for_label(&self, text: &str) -> Option<String> {
        (text == self.original)
            .then(|| self.display.clone())
            .flatten()
    }

    pub(super) fn suppress_label(&self) -> bool {
        !self.show_destination && self.display.is_some()
    }

    pub(super) fn finish(self) -> LinkFinish {
        if self.show_destination {
            return LinkFinish::ShowDestination {
                destination: self.destination,
            };
        }
        match self.display {
            Some(display) => LinkFinish::LocalTarget {
                display,
                destination: self.destination,
            },
            None => LinkFinish::ShowDestination {
                destination: self.destination,
            },
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum LinkFinish {
    ShowDestination {
        destination: String,
    },
    LocalTarget {
        display: String,
        destination: String,
    },
}
