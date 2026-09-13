//! Pointer event handling and widget hit-testing.

use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, QueueHandle};

use crate::render::layout::Rect;
use crate::shell::FlatbarState;

/// Types of mouse interactions on bar widgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetMouseEvent {
    LeftClick,
    RightClick,
    MiddleClick,
    ScrollUp,
    ScrollDown,
}

impl SeatHandler for FlatbarState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        self.current_seat = Some(seat.clone());
        if capability == Capability::Pointer && self.pointer.is_none() {
            if let Ok(pointer) = self.seat_state.get_pointer(qh, &seat) {
                self.pointer = Some(pointer);
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}
}

impl PointerHandler for FlatbarState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let (x, y) = (event.position.0 as i32, event.position.1 as i32);

            match event.kind {
                PointerEventKind::Motion { .. } => {
                    if let Some(ref mut popup) = self.active_popup {
                        popup.handle_pointer_motion(x, y, &self.text_renderer);
                    }
                }
                PointerEventKind::Press { button, serial, .. } => {
                    self.last_pointer_serial = Some(serial);

                    let mouse_event = match button {
                        0x110 => WidgetMouseEvent::LeftClick,   // BTN_LEFT
                        0x111 => WidgetMouseEvent::RightClick,  // BTN_RIGHT
                        0x112 => WidgetMouseEvent::MiddleClick, // BTN_MIDDLE
                        _ => continue,
                    };

                    // Check if clicked inside active popup
                    if let Some(ref mut popup) = self.active_popup {
                        match popup.handle_pointer_click_with_button(
                            x,
                            y,
                            button,
                            &self.text_renderer,
                        ) {
                            crate::shell::popup::PopupClickResult::MenuItem(item_id) => {
                                tracing::debug!("Popup item selected: '{item_id}'");
                                let source_widget_id = self.popup_source_widget.take();
                                self.close_popup();
                                if let Some(wid) = source_widget_id {
                                    if let Some(widget) =
                                        self.widgets.iter().find(|w| w.id() == wid)
                                    {
                                        widget.handle_menu_action(&item_id);
                                    }
                                }
                                self.queue_redraw(qh);
                                continue;
                            }
                            crate::shell::popup::PopupClickResult::TrayAction(action) => {
                                tracing::debug!("Tray action selected: {action:?}");
                                match *action {
                                    crate::shell::popup::TrayClickAction::Activate(item) => {
                                        self.close_popup();
                                        if let Some(ref dbus) = self.dbus_handle {
                                            dbus.activate(&item.service, &item.path, 0, 0);
                                        }
                                    }
                                    crate::shell::popup::TrayClickAction::ContextMenu(item) => {
                                        let source_widget_id = self.popup_source_widget.take();
                                        self.close_popup();
                                        if let Some(ref dbus) = self.dbus_handle {
                                            if let Some(menu_path) = &item.menu_path {
                                                if let Ok(menu_model) =
                                                    dbus.fetch_menu(&item.service, menu_path)
                                                {
                                                    let wid = source_widget_id
                                                        .as_deref()
                                                        .unwrap_or("tray");
                                                    self.open_menu_for_widget(
                                                        qh,
                                                        wid,
                                                        Rect::new(x, y, 1, 1),
                                                        menu_model,
                                                    );
                                                }
                                            } else {
                                                dbus.context_menu(&item.service, &item.path, 0, 0);
                                            }
                                        }
                                    }
                                    crate::shell::popup::TrayClickAction::SecondaryActivate(
                                        item,
                                    ) => {
                                        self.close_popup();
                                        if let Some(ref dbus) = self.dbus_handle {
                                            dbus.secondary_activate(
                                                &item.service,
                                                &item.path,
                                                0,
                                                0,
                                            );
                                        }
                                    }
                                    crate::shell::popup::TrayClickAction::Scroll(item, delta) => {
                                        if let Some(ref dbus) = self.dbus_handle {
                                            dbus.scroll(
                                                &item.service,
                                                &item.path,
                                                delta,
                                                "vertical",
                                            );
                                        }
                                    }
                                }
                                self.queue_redraw(qh);
                                continue;
                            }
                            crate::shell::popup::PopupClickResult::HandledKeepOpen => {
                                self.queue_redraw(qh);
                                continue;
                            }
                            crate::shell::popup::PopupClickResult::Dismiss => {
                                self.close_popup();
                            }
                        }
                    }

                    let hit_info = self.current_layout.hit_test(x, y).map(|wp| {
                        let span_idx = wp.spans.iter().position(|sp| sp.rect.contains(x, y));
                        let rel_x = x - wp.rect.x;
                        let rel_y = y - wp.rect.y;
                        (wp.id.clone(), wp.rect, rel_x, rel_y, span_idx)
                    });

                    if let Some((wp_id, wp_rect, rel_x, rel_y, span_idx)) = hit_info {
                        tracing::debug!(
                            "Pointer click {:?} on widget '{}' span {:?} at ({x}, {y})",
                            mouse_event,
                            wp_id,
                            span_idx
                        );

                        if mouse_event == WidgetMouseEvent::LeftClick {
                            let maybe_popup = self
                                .widgets
                                .iter()
                                .find(|w| w.id() == wp_id)
                                .and_then(|w| w.popup_request());

                            if let Some(req) = maybe_popup {
                                match req {
                                    crate::widget::PopupRequest::Menu(menu) => {
                                        self.open_menu_for_widget(qh, &wp_id, wp_rect, menu);
                                    }
                                    crate::widget::PopupRequest::Calendar {
                                        first_day,
                                        launch_command,
                                    } => {
                                        self.open_calendar_popup(
                                            qh,
                                            &wp_id,
                                            wp_rect,
                                            first_day,
                                            launch_command,
                                        );
                                    }
                                    crate::widget::PopupRequest::TrayGrid => {
                                        self.open_tray_popup(qh, &wp_id, wp_rect);
                                    }
                                }
                                self.queue_redraw(qh);
                                continue;
                            }
                        } else if mouse_event == WidgetMouseEvent::RightClick {
                            let hovered = self.widgets.iter().find(|w| w.id() == wp_id);

                            if let Some(widget) = hovered {
                                if widget.right_click_launch() {
                                    self.queue_redraw(qh);
                                    continue;
                                }
                            }

                            let maybe_menu = self
                                .widgets
                                .iter()
                                .find(|w| w.id() == wp_id)
                                .and_then(|w| w.get_menu());

                            if let Some(menu) = maybe_menu {
                                self.open_menu_for_widget(qh, &wp_id, wp_rect, menu);
                                self.queue_redraw(qh);
                                continue;
                            }
                        }

                        if let Some(widget) = self.widgets.iter().find(|w| w.id() == wp_id) {
                            widget.handle_mouse_event_at(mouse_event, rel_x, rel_y, span_idx);
                            self.queue_redraw(qh);
                        }
                    }
                }
                PointerEventKind::Axis { vertical, .. } => {
                    if vertical.stop {
                        continue;
                    }

                    let direction = if vertical.absolute < 0.0
                        || vertical.discrete < 0
                        || vertical.value120 < 0
                    {
                        -1i32
                    } else if vertical.absolute > 0.0
                        || vertical.discrete > 0
                        || vertical.value120 > 0
                    {
                        1i32
                    } else {
                        continue;
                    };

                    // Route wheel events from an active popup: menu popups
                    // scroll; other kinds ignore them.
                    if let Some(ref mut popup) = self.active_popup {
                        if popup.handle_scroll(direction, &self.text_renderer) {
                            self.queue_redraw(qh);
                        }
                        continue;
                    }

                    let mouse_event = if direction < 0 {
                        WidgetMouseEvent::ScrollUp
                    } else {
                        WidgetMouseEvent::ScrollDown
                    };

                    let hit_info = self.current_layout.hit_test(x, y).map(|wp| {
                        let span_idx = wp.spans.iter().position(|sp| sp.rect.contains(x, y));
                        let rel_x = x - wp.rect.x;
                        let rel_y = y - wp.rect.y;
                        (wp.id.clone(), rel_x, rel_y, span_idx)
                    });

                    if let Some((wp_id, rel_x, rel_y, span_idx)) = hit_info {
                        tracing::debug!(
                            "Pointer scroll {:?} on widget '{}' span {:?} at ({x}, {y})",
                            mouse_event,
                            wp_id,
                            span_idx
                        );
                        if let Some(widget) = self.widgets.iter().find(|w| w.id() == wp_id) {
                            widget.handle_mouse_event_at(mouse_event, rel_x, rel_y, span_idx);
                            self.queue_redraw(qh);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
