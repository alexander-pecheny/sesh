use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Color,
};

use crate::app::state::Palette;

pub(super) fn panel_contrast_fg(palette: &Palette) -> Color {
    match palette.panel_bg {
        Color::Reset => palette.surface_dim,
        color => color,
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ModalStackAreas {
    pub header: Rect,
    pub content: Rect,
    pub footer: Option<Rect>,
    pub actions: Option<Rect>,
}

pub(crate) fn modal_stack_areas(
    inner: Rect,
    header_height: u16,
    footer_height: u16,
    actions_height: u16,
    gap: u16,
) -> ModalStackAreas {
    #[derive(Clone, Copy)]
    enum Slot {
        Header,
        Content,
        Footer,
        Actions,
    }

    let mut constraints = Vec::new();
    let mut slots = Vec::new();
    let mut push = |slot: Slot, constraint: Constraint| {
        if !slots.is_empty() {
            constraints.push(Constraint::Length(gap));
        }
        constraints.push(constraint);
        slots.push(slot);
    };

    push(Slot::Header, Constraint::Length(header_height));
    push(Slot::Content, Constraint::Min(0));
    if footer_height > 0 {
        push(Slot::Footer, Constraint::Length(footer_height));
    }
    if actions_height > 0 {
        push(Slot::Actions, Constraint::Length(actions_height));
    }

    let areas = Layout::vertical(constraints).split(inner);
    let mut result = ModalStackAreas {
        header: Rect::default(),
        content: Rect::default(),
        footer: None,
        actions: None,
    };
    for (slot, area) in slots.into_iter().zip(areas.iter().step_by(2).copied()) {
        match slot {
            Slot::Header => result.header = area,
            Slot::Content => result.content = area,
            Slot::Footer => result.footer = Some(area),
            Slot::Actions => result.actions = Some(area),
        }
    }
    result
}

fn action_button_width(hint: Option<&str>, label: &str) -> u16 {
    match hint {
        Some(hint) => format!(" {hint} {label} ").chars().count() as u16,
        None => format!(" {label} ").chars().count() as u16,
    }
}

pub(crate) fn continue_button_rect(area: Rect) -> Rect {
    Rect::new(
        area.x,
        area.y,
        action_button_width(Some("↵"), "continue"),
        1,
    )
}
