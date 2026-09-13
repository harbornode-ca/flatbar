//! Layer-surface-parented popup surface management, positioner configuration, grab ordering, and dismissal.

use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use smithay_client_toolkit::shell::xdg::popup::Popup;
use smithay_client_toolkit::shell::xdg::{XdgPositioner, XdgShell};
use smithay_client_toolkit::shm::Shm;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::QueueHandle;
use wayland_protocols::xdg::shell::client::xdg_positioner::{
    Anchor, ConstraintAdjustment, Gravity,
};

use crate::config::{Color, Position};
use crate::dbus::sni::SniItem;
use crate::render::buffer::BufferManager;
use crate::render::layout::Rect;
use crate::render::text::TextRenderer;
use crate::shell::scaling::logical_to_buffer;
use crate::shell::FlatbarState;
use crate::widget::calendar::{
    layout_calendar, render_calendar, CalendarHit, CalendarLayout, CalendarModel,
    CalendarRenderParams, FirstDay,
};
use crate::widget::menu::{
    layout_menu_with_limits, render_menu, MenuLayout, MenuModel, MenuRenderParams,
};
use crate::widget::tray::{
    layout_tray_grid, render_tray_grid, TrayGridLayout, TrayGridRenderParams,
};

/// Action to dispatch on a tray icon.
#[derive(Debug, Clone, PartialEq)]
pub enum TrayClickAction {
    Activate(SniItem),
    ContextMenu(SniItem),
    SecondaryActivate(SniItem),
    Scroll(SniItem, i32),
}

/// Result of a click inside an active popup.
#[derive(Debug, Clone, PartialEq)]
pub enum PopupClickResult {
    MenuItem(String),
    TrayAction(Box<TrayClickAction>),
    HandledKeepOpen,
    Dismiss,
}

/// Clamp a menu scroll offset after scrolling `items` item-steps from
/// `current`. Pure helper for scrolling beyond the viewport.
pub fn clamp_scroll_offset(current: u32, items: i32, item_height: u32, max_scroll: u32) -> u32 {
    let next = (current as i64) + items as i64 * item_height as i64;
    next.clamp(0, max_scroll as i64) as u32
}

/// Parameters for opening a menu popup.
pub struct PopupOpenParams<'a> {
    pub compositor_state: &'a CompositorState,
    pub xdg_shell: &'a XdgShell,
    pub layer_surface: &'a LayerSurface,
    pub shm: &'a Shm,
    pub seat: Option<&'a WlSeat>,
    pub serial: Option<u32>,
    pub anchor_rect: Rect,
    pub bar_position: Position,
    pub menu: MenuModel,
    /// Optional width clamp for the menu popup (e.g. ¼ of output width).
    pub max_width: Option<u32>,
    pub font_family: &'a str,
    pub font_size: f32,
    pub scale: f64,
    pub bg_color: Color,
    pub fg_color: Color,
}

/// Parameters for opening a calendar popup.
pub struct CalendarPopupOpenParams<'a> {
    pub compositor_state: &'a CompositorState,
    pub xdg_shell: &'a XdgShell,
    pub layer_surface: &'a LayerSurface,
    pub shm: &'a Shm,
    pub seat: Option<&'a WlSeat>,
    pub serial: Option<u32>,
    pub anchor_rect: Rect,
    pub bar_position: Position,
    pub first_day: FirstDay,
    /// Optional launch command for the button under the calendar grid.
    pub launch_command: Option<Vec<String>>,
    pub font_family: &'a str,
    pub font_size: f32,
    pub scale: f64,
    pub bg_color: Color,
    pub fg_color: Color,
}

/// Parameters for opening a tray icon grid popup.
pub struct TrayGridPopupOpenParams<'a> {
    pub compositor_state: &'a CompositorState,
    pub xdg_shell: &'a XdgShell,
    pub layer_surface: &'a LayerSurface,
    pub shm: &'a Shm,
    pub seat: Option<&'a WlSeat>,
    pub serial: Option<u32>,
    pub anchor_rect: Rect,
    pub bar_position: Position,
    pub items: Vec<SniItem>,
    pub font_family: &'a str,
    pub font_size: f32,
    pub scale: f64,
    pub bg_color: Color,
    pub fg_color: Color,
}

/// The specific content kind contained within the active popup surface.
pub enum PopupKind {
    Menu {
        menu: MenuModel,
        layout: MenuLayout,
        hover_index: Option<usize>,
        /// Scroll offset in logical pixels for menus taller than the viewport.
        scroll_offset: u32,
        /// Height of a single menu item in logical pixels (scroll step).
        item_height: u32,
    },
    Calendar {
        model: CalendarModel,
        layout: CalendarLayout,
        hovered: CalendarHit,
        launch_command: Option<Vec<String>>,
    },
    TrayGrid {
        items: Vec<SniItem>,
        layout: TrayGridLayout,
        hover_index: Option<usize>,
    },
}

/// Manages active popup lifecycle and rendering state.
pub struct ActivePopup {
    pub popup: Popup,
    pub buffer_mgr: BufferManager,
    pub kind: PopupKind,
    pub logical_width: u32,
    pub logical_height: u32,
    pub font_family: String,
    pub font_size: f32,
    pub bg_color: Color,
    pub fg_color: Color,
    pub scale: f64,
}

impl ActivePopup {
    /// Create, parent, and grab a new menu popup below or above a widget.
    /// The surface is committed without content; the first render happens in
    /// `PopupHandler::configure` after the initial configure has been acked.
    pub fn open(
        qh: &QueueHandle<FlatbarState>,
        text_renderer: &TextRenderer,
        params: PopupOpenParams<'_>,
    ) -> Result<Self, String> {
        let layout = layout_menu_with_limits(
            &params.menu,
            text_renderer,
            params.font_family,
            params.font_size,
            params.max_width,
        );

        // Menus scroll once they exceed 10 visible items; otherwise render at
        // full content height.
        let item_height = (params.font_size * 2.0).round().max(26.0) as u32;
        let padding_v = 8u32;
        let viewport_cap = item_height * 10 + padding_v * 2;
        let logical_width = layout.width;
        let logical_height = layout.height.min(viewport_cap);

        let physical_width = logical_to_buffer(logical_width, params.scale);
        let physical_height = logical_to_buffer(logical_height, params.scale);

        let buffer_mgr = BufferManager::new(params.shm, physical_width, physical_height)
            .map_err(|e| format!("Failed to create popup BufferManager: {e}"))?;

        // 1. Create xdg_positioner
        let positioner = XdgPositioner::new(params.xdg_shell)
            .map_err(|e| format!("Failed to create positioner: {e:?}"))?;

        positioner.set_size(logical_width as i32, logical_height as i32);
        positioner.set_anchor_rect(
            params.anchor_rect.x,
            params.anchor_rect.y,
            params.anchor_rect.width as i32,
            params.anchor_rect.height as i32,
        );

        match params.bar_position {
            Position::Top => {
                positioner.set_anchor(Anchor::Bottom);
                positioner.set_gravity(Gravity::Bottom);
            }
            Position::Bottom => {
                positioner.set_anchor(Anchor::Top);
                positioner.set_gravity(Gravity::Top);
            }
        }

        positioner.set_constraint_adjustment(
            ConstraintAdjustment::FlipY
                | ConstraintAdjustment::SlideX
                | ConstraintAdjustment::ResizeY,
        );

        // 2. Create surface & xdg popup
        let surface = params.compositor_state.create_surface(qh);
        let popup = Popup::from_surface(None, &positioner, qh, surface, params.xdg_shell)
            .map_err(|e| format!("Failed to create XDG popup: {e}"))?;

        // 3. Parent popup to layer surface BEFORE grab (Spec C grab discipline)
        params.layer_surface.get_popup(popup.xdg_popup());

        // 4. Request grab if serial and seat available
        if let (Some(s), Some(ser)) = (params.seat, params.serial) {
            popup.xdg_popup().grab(s, ser);
        }

        // 5. Initial commit to map the surface. Content is attached later, in
        // PopupHandler::configure, after the initial configure has been acked
        // (attaching a buffer before that is a protocol error).
        popup.wl_surface().commit();

        Ok(Self {
            popup,
            buffer_mgr,
            kind: PopupKind::Menu {
                menu: params.menu,
                layout,
                hover_index: None,
                scroll_offset: 0,
                item_height,
            },
            logical_width,
            logical_height,
            font_family: params.font_family.to_string(),
            font_size: params.font_size,
            bg_color: params.bg_color,
            fg_color: params.fg_color,
            scale: params.scale,
        })
    }

    /// Create, parent, and grab a new calendar popup below or above the datetime widget.
    pub fn open_calendar(
        qh: &QueueHandle<FlatbarState>,
        text_renderer: &TextRenderer,
        params: CalendarPopupOpenParams<'_>,
    ) -> Result<Self, String> {
        let model = CalendarModel::current(params.first_day);
        let layout = layout_calendar(
            &model,
            text_renderer,
            params.font_family,
            params.font_size,
            params.launch_command.as_deref(),
        );
        let logical_width = layout.width;
        let logical_height = layout.height;

        let physical_width = logical_to_buffer(logical_width, params.scale);
        let physical_height = logical_to_buffer(logical_height, params.scale);

        let buffer_mgr = BufferManager::new(params.shm, physical_width, physical_height)
            .map_err(|e| format!("Failed to create popup BufferManager: {e}"))?;

        let positioner = XdgPositioner::new(params.xdg_shell)
            .map_err(|e| format!("Failed to create positioner: {e:?}"))?;

        positioner.set_size(logical_width as i32, logical_height as i32);
        positioner.set_anchor_rect(
            params.anchor_rect.x,
            params.anchor_rect.y,
            params.anchor_rect.width as i32,
            params.anchor_rect.height as i32,
        );

        match params.bar_position {
            Position::Top => {
                positioner.set_anchor(Anchor::Bottom);
                positioner.set_gravity(Gravity::Bottom);
            }
            Position::Bottom => {
                positioner.set_anchor(Anchor::Top);
                positioner.set_gravity(Gravity::Top);
            }
        }

        positioner.set_constraint_adjustment(
            ConstraintAdjustment::FlipY
                | ConstraintAdjustment::SlideX
                | ConstraintAdjustment::ResizeY,
        );

        let surface = params.compositor_state.create_surface(qh);
        let popup = Popup::from_surface(None, &positioner, qh, surface, params.xdg_shell)
            .map_err(|e| format!("Failed to create XDG popup: {e}"))?;

        params.layer_surface.get_popup(popup.xdg_popup());

        if let (Some(s), Some(ser)) = (params.seat, params.serial) {
            popup.xdg_popup().grab(s, ser);
        }

        // Commit to map the surface; content is attached in PopupHandler::configure
        // once the initial configure has been acked.
        popup.wl_surface().commit();

        Ok(Self {
            popup,
            buffer_mgr,
            kind: PopupKind::Calendar {
                model,
                layout,
                hovered: CalendarHit::None,
                launch_command: params.launch_command,
            },
            logical_width,
            logical_height,
            font_family: params.font_family.to_string(),
            font_size: params.font_size,
            bg_color: params.bg_color,
            fg_color: params.fg_color,
            scale: params.scale,
        })
    }

    /// Create, parent, and grab a new tray grid popup.
    pub fn open_tray_grid(
        qh: &QueueHandle<FlatbarState>,
        text_renderer: &TextRenderer,
        params: TrayGridPopupOpenParams<'_>,
    ) -> Result<Self, String> {
        let layout = layout_tray_grid(
            &params.items,
            text_renderer,
            params.font_family,
            params.font_size,
        );
        let logical_width = layout.width;
        let logical_height = layout.height;

        let physical_width = logical_to_buffer(logical_width, params.scale);
        let physical_height = logical_to_buffer(logical_height, params.scale);

        let buffer_mgr = BufferManager::new(params.shm, physical_width, physical_height)
            .map_err(|e| format!("Failed to create popup BufferManager: {e}"))?;

        let positioner = XdgPositioner::new(params.xdg_shell)
            .map_err(|e| format!("Failed to create positioner: {e:?}"))?;

        positioner.set_size(logical_width as i32, logical_height as i32);
        positioner.set_anchor_rect(
            params.anchor_rect.x,
            params.anchor_rect.y,
            params.anchor_rect.width as i32,
            params.anchor_rect.height as i32,
        );

        match params.bar_position {
            Position::Top => {
                positioner.set_anchor(Anchor::Bottom);
                positioner.set_gravity(Gravity::Bottom);
            }
            Position::Bottom => {
                positioner.set_anchor(Anchor::Top);
                positioner.set_gravity(Gravity::Top);
            }
        }

        positioner.set_constraint_adjustment(
            ConstraintAdjustment::FlipY
                | ConstraintAdjustment::SlideX
                | ConstraintAdjustment::ResizeY,
        );

        let surface = params.compositor_state.create_surface(qh);
        let popup = Popup::from_surface(None, &positioner, qh, surface, params.xdg_shell)
            .map_err(|e| format!("Failed to create XDG popup: {e}"))?;

        params.layer_surface.get_popup(popup.xdg_popup());

        if let (Some(s), Some(ser)) = (params.seat, params.serial) {
            popup.xdg_popup().grab(s, ser);
        }

        // Commit to map the surface; content is attached in PopupHandler::configure
        // once the initial configure has been acked.
        popup.wl_surface().commit();

        Ok(Self {
            popup,
            buffer_mgr,
            kind: PopupKind::TrayGrid {
                items: params.items,
                layout,
                hover_index: None,
            },
            logical_width,
            logical_height,
            font_family: params.font_family.to_string(),
            font_size: params.font_size,
            bg_color: params.bg_color,
            fg_color: params.fg_color,
            scale: params.scale,
        })
    }

    /// Render popup content to its SHM buffer and commit.
    pub fn render(&mut self, text_renderer: &TextRenderer) -> Result<(), String> {
        let physical_width = logical_to_buffer(self.logical_width, self.scale);
        let physical_height = logical_to_buffer(self.logical_height, self.scale);

        if physical_width == 0 || physical_height == 0 {
            return Ok(());
        }

        self.buffer_mgr.resize(physical_width, physical_height);
        let (buffer, canvas) = self
            .buffer_mgr
            .create_buffer()
            .map_err(|e| format!("Failed to create popup draw buffer: {e}"))?;

        let pixels: &mut [u32] = bytemuck::cast_slice_mut(canvas);

        match &self.kind {
            PopupKind::Menu {
                menu,
                layout,
                hover_index,
                scroll_offset,
                item_height: _,
            } => {
                let render_params = MenuRenderParams {
                    menu,
                    layout,
                    hover_index: *hover_index,
                    font_family: &self.font_family,
                    font_size: self.font_size,
                    scale: self.scale,
                    bg_color: self.bg_color,
                    fg_color: self.fg_color,
                    scroll_offset: *scroll_offset,
                };
                render_menu(
                    pixels,
                    physical_width,
                    physical_height,
                    text_renderer,
                    &render_params,
                );
            }
            PopupKind::Calendar {
                model,
                layout,
                hovered,
                launch_command: _,
            } => {
                let render_params = CalendarRenderParams {
                    model,
                    layout,
                    hovered: *hovered,
                    font_family: &self.font_family,
                    font_size: self.font_size,
                    scale: self.scale,
                    bg_color: self.bg_color,
                    fg_color: self.fg_color,
                };
                render_calendar(
                    pixels,
                    physical_width,
                    physical_height,
                    text_renderer,
                    &render_params,
                );
            }
            PopupKind::TrayGrid {
                items,
                layout,
                hover_index,
            } => {
                let render_params = TrayGridRenderParams {
                    items,
                    layout,
                    hover_index: *hover_index,
                    font_family: &self.font_family,
                    font_size: self.font_size,
                    scale: self.scale,
                    bg_color: self.bg_color,
                    fg_color: self.fg_color,
                };
                render_tray_grid(
                    pixels,
                    physical_width,
                    physical_height,
                    text_renderer,
                    &render_params,
                );
            }
        }

        let wl_surface = self.popup.wl_surface();
        buffer
            .attach_to(wl_surface)
            .map_err(|e| format!("Failed to attach popup buffer: {e}"))?;

        wl_surface.damage_buffer(0, 0, physical_width as i32, physical_height as i32);
        wl_surface.commit();
        Ok(())
    }

    /// Adjust a point expressed in viewport coordinates into full-menu
    /// coordinates by adding the scroll offset.
    fn menu_point_with_scroll(scroll_offset: u32, x: i32, y: i32) -> (i32, i32) {
        (x, y + scroll_offset as i32)
    }

    /// Scroll a menu popup by `items` steps (positive scrolls down). Returns
    /// whether a re-render occurred.
    pub fn handle_scroll(&mut self, items: i32, text_renderer: &TextRenderer) -> bool {
        match &mut self.kind {
            PopupKind::Menu {
                layout,
                scroll_offset,
                item_height,
                ..
            } => {
                let max_scroll = layout.height.saturating_sub(self.logical_height);
                let next = clamp_scroll_offset(*scroll_offset, items, *item_height, max_scroll);
                if next != *scroll_offset {
                    *scroll_offset = next;
                    let _ = self.render(text_renderer);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Handle pointer motion over popup.
    pub fn handle_pointer_motion(&mut self, x: i32, y: i32, text_renderer: &TextRenderer) -> bool {
        match &mut self.kind {
            PopupKind::Menu {
                layout,
                hover_index,
                scroll_offset,
                ..
            } => {
                let (x, y) = Self::menu_point_with_scroll(*scroll_offset, x, y);
                let new_hover = layout.hit_test(x, y);
                if new_hover != *hover_index {
                    *hover_index = new_hover;
                    let _ = self.render(text_renderer);
                    true
                } else {
                    false
                }
            }
            PopupKind::Calendar {
                layout, hovered, ..
            } => {
                let new_hit = layout.hit_test(x, y);
                if new_hit != *hovered {
                    *hovered = new_hit;
                    let _ = self.render(text_renderer);
                    true
                } else {
                    false
                }
            }
            PopupKind::TrayGrid {
                layout,
                hover_index,
                ..
            } => {
                let new_hover = layout.hit_test(x, y);
                if new_hover != *hover_index {
                    *hover_index = new_hover;
                    let _ = self.render(text_renderer);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Handle pointer click on popup with a specific button (e.g. BTN_LEFT, BTN_RIGHT, BTN_MIDDLE).
    pub fn handle_pointer_click_with_button(
        &mut self,
        x: i32,
        y: i32,
        button: u32,
        text_renderer: &TextRenderer,
    ) -> PopupClickResult {
        match &mut self.kind {
            PopupKind::Menu {
                menu,
                layout,
                scroll_offset,
                ..
            } => {
                let (x, y) = Self::menu_point_with_scroll(*scroll_offset, x, y);
                if let Some(idx) = layout.hit_test(x, y) {
                    if idx < menu.items.len() {
                        let item = &menu.items[idx];
                        if item.is_enabled() {
                            if let Some(id) = item.id() {
                                return PopupClickResult::MenuItem(id.to_string());
                            }
                        }
                    }
                }
                PopupClickResult::Dismiss
            }
            PopupKind::Calendar {
                model,
                layout,
                launch_command,
                ..
            } => {
                let hit = layout.hit_test(x, y);
                match hit {
                    CalendarHit::PrevMonth => {
                        model.prev_month();
                        *layout = layout_calendar(
                            model,
                            text_renderer,
                            &self.font_family,
                            self.font_size,
                            launch_command.as_deref(),
                        );
                        let _ = self.render(text_renderer);
                        PopupClickResult::HandledKeepOpen
                    }
                    CalendarHit::NextMonth => {
                        model.next_month();
                        *layout = layout_calendar(
                            model,
                            text_renderer,
                            &self.font_family,
                            self.font_size,
                            launch_command.as_deref(),
                        );
                        let _ = self.render(text_renderer);
                        PopupClickResult::HandledKeepOpen
                    }
                    CalendarHit::Launcher => {
                        PopupClickResult::MenuItem("calendar-launch".to_string())
                    }
                    CalendarHit::Day(_) => PopupClickResult::HandledKeepOpen,
                    CalendarHit::None => PopupClickResult::Dismiss,
                }
            }
            PopupKind::TrayGrid { items, layout, .. } => {
                if let Some(idx) = layout.hit_test(x, y) {
                    if idx < items.len() {
                        let item = items[idx].clone();
                        match button {
                            0x110 => {
                                return PopupClickResult::TrayAction(Box::new(
                                    TrayClickAction::Activate(item),
                                ))
                            }
                            0x111 => {
                                return PopupClickResult::TrayAction(Box::new(
                                    TrayClickAction::ContextMenu(item),
                                ))
                            }
                            0x112 => {
                                return PopupClickResult::TrayAction(Box::new(
                                    TrayClickAction::SecondaryActivate(item),
                                ))
                            }
                            _ => {
                                return PopupClickResult::TrayAction(Box::new(
                                    TrayClickAction::Activate(item),
                                ))
                            }
                        }
                    }
                }
                PopupClickResult::Dismiss
            }
        }
    }

    /// Handle pointer click on popup. Returns the action result.
    pub fn handle_pointer_click(
        &mut self,
        x: i32,
        y: i32,
        text_renderer: &TextRenderer,
    ) -> PopupClickResult {
        self.handle_pointer_click_with_button(x, y, 0x110, text_renderer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clamp_scroll_offset_beyond_viewport() {
        // 15 items * 26px = 390px content, viewport 10 items = 260px + padding.
        let item = 26u32;
        let max = 130u32; // scrollable extent

        // Step down twice from idle.
        assert_eq!(clamp_scroll_offset(0, 1, item, max), 26);
        assert_eq!(clamp_scroll_offset(26, 1, item, max), 52);
        // Reaching and staying within the end clamp.
        assert_eq!(clamp_scroll_offset(104, 5, item, max), max);
        // Scrolling past the end saturates.
        assert_eq!(clamp_scroll_offset(max, 1, item, max), max);
        // Scrolling above the start saturates.
        assert_eq!(clamp_scroll_offset(0, -3, item, max), 0);
        // Short menu: nothing to scroll.
        assert_eq!(clamp_scroll_offset(0, 1, item, 0), 0);
    }
}
