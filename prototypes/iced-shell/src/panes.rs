//! Terminal-shaped fixture and utility pane rendering; no backend operations.
use super::*;
use iced::Font;

impl App {
    pub(super) fn terminals(&self) -> Element<'_, Message> {
        let tab = &self.tabs[self.active];
        pane_grid(&tab.panes, |id, profile, _| {
            let focused = tab.focus == id;
            let title = pane_grid::TitleBar::new(row![
                text(if focused { "FOCUSED" } else { "SSH" }).size(11).color(if focused { BLUE } else { MUTED }),
                text(format!("{}@{}", profile.user, profile.name)).size(13),
                space::horizontal(), action("x", Message::ClosePane(id)),
            ].spacing(10).align_y(iced::Center)).padding([4, 10]).style(surface);
            let transcript = format!("{}@{}:~$ pwd\n/home/{}\n\n{}@{}:~$ ls -lh\ntotal 24K\ndrwxr-xr-x  4 ops ops  4.0K  config/\ndrwxr-xr-x  2 ops ops  4.0K  logs/\n-rw-r--r--  1 ops ops   892  deployment.yml\n\n{}@{}:~$ ", profile.user, profile.name, profile.user, profile.user, profile.name, profile.user, profile.name);
            // Fill the actual pane width, not only the text's intrinsic width.
            // Do not place an unbounded Fill spacer inside a vertical scrollable.
            let body = column![
                text("SIMULATED TERMINAL / sample output, not a live shell").size(12).color(MUTED),
                text(transcript).font(Font::MONOSPACE).size(15).color(FG),
                space::vertical().height(12),
                text("Drag the pane header to rearrange. Drag dividers to resize.").size(12).color(MUTED),
            ].spacing(18).width(Fill);
            pane_grid::Content::new(container(scrollable(body).width(Fill).height(Fill)).padding(18).width(Fill).height(Fill))
                .title_bar(title).style(if focused { active_card } else { card })
        }).spacing(8).on_click(Message::Focus).on_drag(Message::Drag).on_resize(8, Message::Resize).height(Fill).into()
    }

    pub(super) fn files(&self) -> Element<'_, Message> {
        let tab = &self.tabs[self.active];
        let mut remote_rows = column![
            text(format!("REMOTE / {}", tab.profile.name)).size(12).color(BLUE),
            text("/home/ops  /  sample files").size(13).color(MUTED),
        ].spacing(6).width(Fill);
        for (index, (name, size)) in [("deployment.yml", "892 B"), ("notes.txt", "1.2 KB"), ("service.log", "12.4 KB")].iter().enumerate() {
            remote_rows = remote_rows.push(button(row![text(*name), space::horizontal(), text(*size).size(12).color(MUTED)])
                .padding([6, 8]).width(Fill).on_press(Message::File(index))
                .style(if self.selected_file == Some(index) { selected_button } else { quiet }));
        }
        let local = column![
            text("LOCAL / sample computer").size(12).color(MUTED),
            text("~/Downloads").size(13).color(MUTED),
            text("Select a remote sample file to preview a transfer.\nNo real files are read or written.").size(13).color(MUTED),
        ].spacing(8).width(Fill);
        let transfer = button(text("Queue preview transfer")).padding([7, 12]).style(quiet)
            .on_press_maybe(self.selected_file.map(|_| Message::QueuePreview));
        let header = row![text("Files").size(16), text("SFTP design preview").size(12).color(MUTED), space::horizontal(), transfer]
            .spacing(12).align_y(iced::Center);
        // File listings scroll independently. The queue remains visible below them.
        let explorers = row![
            container(scrollable(local).width(Fill).height(Fill)).width(Fill).height(Fill).padding([0, 10]),
            container(scrollable(remote_rows).width(Fill).height(Fill)).width(Fill).height(Fill).padding([0, 10]),
        ].spacing(12).height(Fill);
        let queue_text = if self.preview_transfers.is_empty() { "Transfer queue is empty".into() } else { self.preview_transfers.join("\n") };
        let queue = container(column![
            text(format!("TRANSFERS ({})", self.preview_transfers.len())).size(11).color(MUTED),
            scrollable(text(queue_text).size(12).color(MUTED)).width(Fill).height(Fill),
        ].spacing(4)).padding([7, 10]).height(60).width(Fill).style(surface);
        container(column![header, explorers, queue].spacing(10).height(Fill))
            .padding(12).height(Fill).width(Fill).style(card).into()
    }
}
