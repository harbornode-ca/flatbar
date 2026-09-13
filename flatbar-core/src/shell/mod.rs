//! Wayland client connection, layer surface lifecycle, outputs, fractional scaling, and redraw coordination.

pub mod launcher;
pub mod pointer;
pub mod popup;
pub mod scaling;

use std::time::Duration;

use calloop::signals::{Signal, Signals};
use calloop::timer::{TimeoutAction, Timer};
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryHandler, RegistryState};
use smithay_client_toolkit::seat::SeatState;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_registry, registry_handlers};

use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::config::{Color, Config, Position};
use crate::render::buffer::BufferManager;
use crate::render::damage::DamageTracker;
use crate::render::glyphs::render_layout;
use crate::render::layout::{compute_layout, LayoutResult};
use crate::render::text::TextRenderer;
use crate::shell::popup::ActivePopup;
use crate::shell::scaling::logical_to_buffer;
use crate::widget::Widget;

/// Main application runtime state.
pub struct FlatbarState {
    pub config: Config,
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub compositor_state: CompositorState,
    pub seat_state: SeatState,
    pub shm: Shm,
    pub layer_shell: Option<LayerShell>,
    pub xdg_shell: Option<XdgShell>,
    pub fractional_scale_mgr: Option<WpFractionalScaleManagerV1>,
    pub viewporter: Option<WpViewporter>,

    pub pointer: Option<WlPointer>,
    pub current_seat: Option<WlSeat>,
    pub last_pointer_serial: Option<u32>,
    pub target_output: Option<WlOutput>,
    pub layer_surface: Option<LayerSurface>,
    pub viewport: Option<WpViewport>,
    pub fractional_scale: Option<WpFractionalScaleV1>,
    pub active_popup: Option<ActivePopup>,
    pub popup_source_widget: Option<String>,
    pub launcher_tx: Option<calloop::channel::Sender<(String, String)>>,
    pub dbus_handle: Option<crate::dbus::bridge::DbusBridgeHandle>,

    pub buffer_mgr: Option<BufferManager>,
    pub text_renderer: TextRenderer,
    pub damage_tracker: DamageTracker,
    pub current_layout: LayoutResult,

    pub logical_width: u32,
    pub logical_height: u32,
    pub scale: f64,
    pub configured: bool,
    pub exit: bool,
    pub waiting_for_frame: bool,
    pub frame_scheduled: bool,

    pub widgets: Vec<Box<dyn Widget>>,
}

impl FlatbarState {
    pub fn new(
        config: Config,
        registry_state: RegistryState,
        compositor_state: CompositorState,
        output_state: OutputState,
        seat_state: SeatState,
        shm: Shm,
    ) -> Self {
        let height = config.bar.height;
        Self {
            config,
            registry_state,
            output_state,
            compositor_state,
            seat_state,
            shm,
            layer_shell: None,
            xdg_shell: None,
            fractional_scale_mgr: None,
            viewporter: None,
            pointer: None,
            current_seat: None,
            last_pointer_serial: None,
            target_output: None,
            layer_surface: None,
            viewport: None,
            fractional_scale: None,
            active_popup: None,
            popup_source_widget: None,
            launcher_tx: None,
            dbus_handle: None,
            buffer_mgr: None,
            text_renderer: TextRenderer::new(),
            damage_tracker: DamageTracker::new(),
            current_layout: LayoutResult::default(),
            logical_width: 0,
            logical_height: height,
            scale: 1.0,
            configured: false,
            exit: false,
            waiting_for_frame: false,
            frame_scheduled: false,
            widgets: Vec::new(),
        }
    }

    /// Close any open popup cleanly.
    pub fn close_popup(&mut self) {
        if self.active_popup.is_some() {
            tracing::debug!("Closing active popup surface");
            self.active_popup = None;
            self.popup_source_widget = None;
        }
    }

    /// Open a menu for a widget using either external launcher or internal popup depending on config.
    pub fn open_menu_for_widget(
        &mut self,
        qh: &QueueHandle<FlatbarState>,
        widget_id: &str,
        anchor_rect: crate::render::layout::Rect,
        menu: crate::widget::menu::MenuModel,
    ) {
        if self.config.bar.menu_backend == crate::config::MenuBackend::Launcher {
            let menu_cmd = self.config.bar.resolved_menu_command();
            if let Some(ref tx) = self.launcher_tx {
                crate::shell::launcher::spawn_menu_launcher(menu_cmd, widget_id, menu, tx.clone());
            }
        } else {
            self.open_widget_popup(qh, widget_id, anchor_rect, menu);
        }
    }

    /// Open a popup menu anchored to the specified widget.
    pub fn open_widget_popup(
        &mut self,
        qh: &QueueHandle<FlatbarState>,
        widget_id: &str,
        anchor_rect: crate::render::layout::Rect,
        menu: crate::widget::menu::MenuModel,
    ) {
        if self.active_popup.is_some() {
            self.close_popup();
            return;
        }

        let xdg_shell = match self.xdg_shell.as_ref() {
            Some(x) => x,
            None => {
                tracing::warn!("XDG shell not available for popup");
                return;
            }
        };

        let layer_surface = match self.layer_surface.as_ref() {
            Some(ls) => ls,
            None => return,
        };

        let bg_color =
            Color::parse_hex(&self.config.bar.background).unwrap_or(Color::rgb(30, 30, 46));
        let fg_color =
            Color::parse_hex(&self.config.bar.foreground).unwrap_or(Color::rgb(205, 214, 244));

        let open_params = crate::shell::popup::PopupOpenParams {
            compositor_state: &self.compositor_state,
            xdg_shell,
            layer_surface,
            shm: &self.shm,
            seat: self.current_seat.as_ref(),
            serial: self.last_pointer_serial,
            anchor_rect,
            bar_position: self.config.bar.position,
            menu,
            // Menu popups are content-fit but capped at ¼ of the output width.
            max_width: Some(crate::shell::scaling::quarter_output_width(
                self.logical_width,
            )),
            font_family: &self.config.bar.font_family,
            font_size: self.config.bar.font_size,
            scale: self.scale,
            bg_color,
            fg_color,
        };

        match ActivePopup::open(qh, &self.text_renderer, open_params) {
            Ok(popup) => {
                self.popup_source_widget = Some(widget_id.to_string());
                self.active_popup = Some(popup);
            }
            Err(e) => {
                tracing::error!("Failed to open popup for widget '{widget_id}': {e}");
            }
        }
    }

    /// Open an interactive calendar popup anchored to the datetime widget.
    pub fn open_calendar_popup(
        &mut self,
        qh: &QueueHandle<FlatbarState>,
        widget_id: &str,
        anchor_rect: crate::render::layout::Rect,
        first_day: crate::widget::calendar::FirstDay,
        launch_command: Option<Vec<String>>,
    ) {
        if self.active_popup.is_some() {
            self.close_popup();
            return;
        }

        let xdg_shell = match self.xdg_shell.as_ref() {
            Some(x) => x,
            None => {
                tracing::warn!("XDG shell not available for popup");
                return;
            }
        };

        let layer_surface = match self.layer_surface.as_ref() {
            Some(ls) => ls,
            None => return,
        };

        let bg_color =
            Color::parse_hex(&self.config.bar.background).unwrap_or(Color::rgb(30, 30, 46));
        let fg_color =
            Color::parse_hex(&self.config.bar.foreground).unwrap_or(Color::rgb(205, 214, 244));

        let open_params = crate::shell::popup::CalendarPopupOpenParams {
            compositor_state: &self.compositor_state,
            xdg_shell,
            layer_surface,
            shm: &self.shm,
            seat: self.current_seat.as_ref(),
            serial: self.last_pointer_serial,
            anchor_rect,
            bar_position: self.config.bar.position,
            first_day,
            launch_command,
            font_family: &self.config.bar.font_family,
            font_size: self.config.bar.font_size,
            scale: self.scale,
            bg_color,
            fg_color,
        };

        match ActivePopup::open_calendar(qh, &self.text_renderer, open_params) {
            Ok(popup) => {
                self.popup_source_widget = Some(widget_id.to_string());
                self.active_popup = Some(popup);
            }
            Err(e) => {
                tracing::error!("Failed to open calendar popup for widget '{widget_id}': {e}");
            }
        }
    }

    /// Open an interactive tray icon grid popup anchored to the tray widget.
    pub fn open_tray_popup(
        &mut self,
        qh: &QueueHandle<FlatbarState>,
        widget_id: &str,
        anchor_rect: crate::render::layout::Rect,
    ) {
        if self.active_popup.is_some() {
            self.close_popup();
            return;
        }

        let xdg_shell = match self.xdg_shell.as_ref() {
            Some(x) => x,
            None => {
                tracing::warn!("XDG shell not available for popup");
                return;
            }
        };

        let layer_surface = match self.layer_surface.as_ref() {
            Some(ls) => ls,
            None => return,
        };

        let items = self
            .widgets
            .iter()
            .find(|w| w.id() == widget_id)
            .and_then(|w| w.as_tray().map(|t| t.visible_items()))
            .unwrap_or_default();

        let bg_color =
            Color::parse_hex(&self.config.bar.background).unwrap_or(Color::rgb(30, 30, 46));
        let fg_color =
            Color::parse_hex(&self.config.bar.foreground).unwrap_or(Color::rgb(205, 214, 244));

        let open_params = crate::shell::popup::TrayGridPopupOpenParams {
            compositor_state: &self.compositor_state,
            xdg_shell,
            layer_surface,
            shm: &self.shm,
            seat: self.current_seat.as_ref(),
            serial: self.last_pointer_serial,
            anchor_rect,
            bar_position: self.config.bar.position,
            items,
            font_family: &self.config.bar.font_family,
            font_size: self.config.bar.font_size,
            scale: self.scale,
            bg_color,
            fg_color,
        };

        match ActivePopup::open_tray_grid(qh, &self.text_renderer, open_params) {
            Ok(popup) => {
                self.popup_source_widget = Some(widget_id.to_string());
                self.active_popup = Some(popup);
            }
            Err(e) => {
                tracing::error!("Failed to open tray grid popup for widget '{widget_id}': {e}");
            }
        }
    }

    /// Reload configuration from file and hot-rebuild widgets and styling.
    pub fn reload_config(&mut self, path: &std::path::Path, qh: &QueueHandle<FlatbarState>) {
        tracing::info!("Reloading configuration from {}", path.display());
        match Config::load_from_file(path) {
            Ok(new_config) => {
                self.config = new_config;
                self.widgets = build_widgets(&self.config);
                self.damage_tracker.invalidate_all();
                self.queue_redraw(qh);
                tracing::info!("Configuration successfully reloaded");
            }
            Err(e) => {
                tracing::error!("Configuration reload failed: {e}");
                let _ = crate::dbus::notify::send_notification(
                    "Flatbar",
                    &format!("Config reload error in {}:\n{e}", path.display()),
                    "dialog-error",
                );
            }
        }
    }

    /// Select appropriate output and set up layer surface.
    pub fn setup_surface(&mut self, qh: &QueueHandle<FlatbarState>) -> Result<(), String> {
        let layer_shell = self
            .layer_shell
            .as_ref()
            .ok_or_else(|| "Compositor does not support wlr-layer-shell".to_string())?;

        // Find matching output or first available output
        let selected_output = self
            .output_state
            .outputs()
            .find(|output| {
                if let Some(info) = self.output_state.info(output) {
                    if let Some(name) = info.name {
                        return self.config.bar.output.matches(&name);
                    }
                }
                false
            })
            .or_else(|| self.output_state.outputs().next());

        self.target_output = selected_output.clone();

        // Update integer scale from output info if fractional scale is not available yet
        if let Some(ref out) = selected_output {
            if let Some(info) = self.output_state.info(out) {
                self.scale = info.scale_factor as f64;
            }
        }

        let surface = self.compositor_state.create_surface(qh);

        let layer = Layer::Top;
        let layer_surface = layer_shell.create_layer_surface(
            qh,
            surface,
            layer,
            Some(format!("org.flatbar.{}", self.config.bar.name)),
            selected_output.as_ref(),
        );

        let mut anchor = Anchor::LEFT | Anchor::RIGHT;
        match self.config.bar.position {
            Position::Top => anchor |= Anchor::TOP,
            Position::Bottom => anchor |= Anchor::BOTTOM,
        }

        layer_surface.set_anchor(anchor);
        layer_surface.set_size(0, self.config.bar.height);
        layer_surface.set_exclusive_zone(self.config.bar.height as i32);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.commit();

        // Setup wp_fractional_scale_v1 if manager is present
        if let Some(ref mgr) = self.fractional_scale_mgr {
            let frac_scale = mgr.get_fractional_scale(layer_surface.wl_surface(), qh, ());
            self.fractional_scale = Some(frac_scale);
        }

        // Setup wp_viewport if viewporter is present
        if let Some(ref viewporter) = self.viewporter {
            let viewport = viewporter.get_viewport(layer_surface.wl_surface(), qh, ());
            self.viewport = Some(viewport);
        }

        self.layer_surface = Some(layer_surface);
        Ok(())
    }

    /// Render a single frame.
    pub fn draw(&mut self, qh: &QueueHandle<FlatbarState>) {
        if !self.configured || self.logical_width == 0 || self.logical_height == 0 {
            return;
        }

        let layer_surface = match self.layer_surface.as_ref() {
            Some(s) => s,
            None => return,
        };

        let physical_width = logical_to_buffer(self.logical_width, self.scale);
        let physical_height = logical_to_buffer(self.logical_height, self.scale);

        if physical_width == 0 || physical_height == 0 {
            return;
        }

        // 1. Compute new layout
        let new_layout = compute_layout(
            &self.config.bar.layout,
            &self.widgets,
            &self.text_renderer,
            &self.config.bar.font_family,
            self.config.bar.font_size,
            self.logical_width,
            self.logical_height,
            self.config.bar.inner_padding,
        );

        // 2. Compute damage
        let dirty_rects = self.damage_tracker.compute_damage(&new_layout);
        if dirty_rects.is_empty() {
            return;
        }

        self.current_layout = new_layout;

        // Ensure buffer manager is created and properly sized
        if self.buffer_mgr.is_none() {
            match BufferManager::new(&self.shm, physical_width, physical_height) {
                Ok(mgr) => self.buffer_mgr = Some(mgr),
                Err(err) => {
                    tracing::error!("Failed to create BufferManager: {err}");
                    return;
                }
            }
        } else if let Some(ref mut mgr) = self.buffer_mgr {
            mgr.resize(physical_width, physical_height);
        }

        let buffer_mgr = self.buffer_mgr.as_mut().unwrap();
        let (buffer, canvas) = match buffer_mgr.create_buffer() {
            Ok(res) => res,
            Err(err) => {
                tracing::error!("Failed to allocate draw buffer: {err}");
                return;
            }
        };

        // Fill background
        let bg_color =
            Color::parse_hex(&self.config.bar.background).unwrap_or(Color::rgb(30, 30, 46));
        BufferManager::clear_canvas(canvas, bg_color);

        // Convert byte canvas slice to u32 pixel slice for drawing
        let pixels: &mut [u32] = bytemuck::cast_slice_mut(canvas);

        let fg_color =
            Color::parse_hex(&self.config.bar.foreground).unwrap_or(Color::rgb(205, 214, 244));

        // Render widgets from layout
        let render_opts = crate::render::glyphs::RenderLayoutOptions {
            font_family: &self.config.bar.font_family,
            font_size: self.config.bar.font_size,
            scale: self.scale,
            bg_color,
            fg_color,
        };
        render_layout(
            pixels,
            physical_width,
            physical_height,
            &self.current_layout,
            &self.text_renderer,
            &render_opts,
        );

        // Set viewport destination to logical size if viewporter is available
        if let Some(ref viewport) = self.viewport {
            viewport.set_destination(self.logical_width as i32, self.logical_height as i32);
        }

        // Attach buffer, apply damaged regions, and commit
        let wl_surface = layer_surface.wl_surface();
        buffer
            .attach_to(wl_surface)
            .expect("Failed to attach buffer");

        let buffer_damage = DamageTracker::to_buffer_damage(&dirty_rects, self.scale);
        for (dx, dy, dw, dh) in buffer_damage {
            wl_surface.damage_buffer(dx, dy, dw as i32, dh as i32);
        }

        // Request frame callback for smooth pacing
        self.waiting_for_frame = true;
        wl_surface.frame(qh, wl_surface.clone());
        wl_surface.commit();
    }

    /// Schedule a redraw on next frame or immediately if idle.
    pub fn queue_redraw(&mut self, qh: &QueueHandle<FlatbarState>) {
        if !self.waiting_for_frame {
            self.draw(qh);
        } else {
            self.frame_scheduled = true;
        }
    }
}

// Implement SCT required traits
impl ProvidesRegistryState for FlatbarState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

impl CompositorHandler for FlatbarState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        new_factor: i32,
    ) {
        self.close_popup();
        if self.fractional_scale.is_none() {
            tracing::debug!("Integer scale factor changed to {new_factor}");
            self.scale = new_factor as f64;
            self.buffer_mgr = None; // Invalidate buffers on scale change
            self.damage_tracker.invalidate_all();
            self.text_renderer.shaping_cache.clear();
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_transform: wayland_client::protocol::wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _time: u32,
    ) {
        self.waiting_for_frame = false;
        if self.frame_scheduled {
            self.frame_scheduled = false;
            self.draw(qh);
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {
    }
}

impl OutputHandler for FlatbarState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {}

    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: WlOutput) {
    }
}

impl ShmHandler for FlatbarState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl LayerShellHandler for FlatbarState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        tracing::info!("Layer surface closed by compositor");
        self.close_popup();
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        tracing::debug!(
            "Layer surface configure: width={}, height={}",
            configure.new_size.0,
            configure.new_size.1
        );
        self.logical_width = configure.new_size.0;
        self.logical_height = if configure.new_size.1 > 0 {
            configure.new_size.1
        } else {
            self.config.bar.height
        };
        self.configured = true;
        self.damage_tracker.invalidate_all();
        self.draw(qh);
    }
}

impl smithay_client_toolkit::shell::xdg::popup::PopupHandler for FlatbarState {
    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _popup: &smithay_client_toolkit::shell::xdg::popup::Popup,
        config: smithay_client_toolkit::shell::xdg::popup::PopupConfigure,
    ) {
        // SCT acks the configure before this call; only now is it legal to
        // attach a buffer to the popup surface (initial configure must be
        // acked first, otherwise the compositor raises a protocol error).
        if let Some(ref mut active) = self.active_popup {
            if config.width > 0 {
                active.logical_width = config.width as u32;
            }
            if config.height > 0 {
                active.logical_height = config.height as u32;
            }
            if let Err(e) = active.render(&self.text_renderer) {
                tracing::error!("Failed to render popup after configure: {e}");
            }
        }
    }

    fn done(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _popup: &smithay_client_toolkit::shell::xdg::popup::Popup,
    ) {
        self.close_popup();
    }
}

impl smithay_client_toolkit::shell::xdg::window::WindowHandler for FlatbarState {
    fn request_close(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _window: &smithay_client_toolkit::shell::xdg::window::Window,
    ) {
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _window: &smithay_client_toolkit::shell::xdg::window::Window,
        _configure: smithay_client_toolkit::shell::xdg::window::WindowConfigure,
        _serial: u32,
    ) {
    }
}

// Registry handler to bind Fractional Scale and Viewporter globals
impl RegistryHandler<FlatbarState> for FlatbarState {
    fn new_global(
        _data: &mut FlatbarState,
        _conn: &Connection,
        _qh: &QueueHandle<FlatbarState>,
        _name: u32,
        _interface: &str,
        _version: u32,
    ) {
    }

    fn remove_global(
        _data: &mut FlatbarState,
        _conn: &Connection,
        _qh: &QueueHandle<FlatbarState>,
        _name: u32,
        _interface: &str,
    ) {
    }
}

// Dispatch implementations for Fractional Scale and Viewport
impl Dispatch<WpFractionalScaleManagerV1, ()> for FlatbarState {
    fn event(
        _state: &mut Self,
        _proxy: &WpFractionalScaleManagerV1,
        _event: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for FlatbarState {
    fn event(
        state: &mut Self,
        _proxy: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let factor = (scale as f64) / 120.0;
            tracing::debug!("Preferred fractional scale received: {factor} ({scale}/120)");
            if (state.scale - factor).abs() > 0.001 {
                state.scale = factor;
                state.buffer_mgr = None; // Recreate buffer pool at new physical size
                state.damage_tracker.invalidate_all();
                state.text_renderer.shaping_cache.clear();
                state.queue_redraw(qh);
            }
        }
    }
}

impl Dispatch<WpViewporter, ()> for FlatbarState {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewporter,
        _event: <WpViewporter as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewport, ()> for FlatbarState {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewport,
        _event: <WpViewport as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlCallback, WlSurface> for FlatbarState {
    fn event(
        state: &mut Self,
        _proxy: &WlCallback,
        _event: wl_callback::Event,
        _data: &WlSurface,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        state.waiting_for_frame = false;
        if state.frame_scheduled {
            state.frame_scheduled = false;
            state.draw(qh);
        }
    }
}

delegate_registry!(FlatbarState);
smithay_client_toolkit::delegate_dispatch2!(FlatbarState);

/// Run the Flatbar Wayland status bar event loop.
pub fn run_flatbar(config: Config) -> Result<(), String> {
    run_flatbar_with_path(config, None)
}

/// Run the Flatbar Wayland status bar event loop with optional config path for live SIGHUP reload.
pub fn run_flatbar_with_path(
    mut config: Config,
    config_path: Option<std::path::PathBuf>,
) -> Result<(), String> {
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("Failed to connect to Wayland compositor: {e}"))?;

    let (globals, event_queue) = registry_queue_init::<FlatbarState>(&conn)
        .map_err(|e| format!("Failed to initialize Wayland registry queue: {e}"))?;
    let qh = event_queue.handle();

    let compositor_state = CompositorState::bind(&globals, &qh)
        .map_err(|e| format!("Failed to bind wl_compositor: {e}"))?;
    let output_state = OutputState::new(&globals, &qh);
    let seat_state = SeatState::new(&globals, &qh);
    let shm = Shm::bind(&globals, &qh).map_err(|e| format!("Failed to bind wl_shm: {e}"))?;
    let registry_state = RegistryState::new(&globals);

    let layer_shell = LayerShell::bind(&globals, &qh).ok();
    let xdg_shell = XdgShell::bind(&globals, &qh).ok();
    let fractional_scale_mgr: Option<WpFractionalScaleManagerV1> =
        globals.bind(&qh, 1..=1, ()).ok();
    let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();

    // Default layout if unspecified
    if config.bar.layout.left.is_empty()
        && config.bar.layout.center.is_empty()
        && config.bar.layout.right.is_empty()
    {
        config.bar.layout.center.push("datetime".to_string());
    }

    let mut state = FlatbarState::new(
        config.clone(),
        registry_state,
        compositor_state,
        output_state,
        seat_state,
        shm,
    );

    state.layer_shell = layer_shell;
    state.xdg_shell = xdg_shell;
    state.fractional_scale_mgr = fractional_scale_mgr;
    state.viewporter = viewporter;

    let widgets = build_widgets(&config);
    let mut widget_intervals = Vec::new();
    for w in &widgets {
        let iv = w.update_interval();
        let wid = w.id().to_string();
        if !iv.is_zero() {
            widget_intervals.push((wid, iv));
        }
    }

    // Channel for widgets to wake the event loop when fresh state is available
    // (e.g. a command widget's async output finishing between timer ticks).
    let (widget_wake_tx, widget_wake_rx) = calloop::channel::channel::<()>();
    let widgets: Vec<Box<dyn Widget>> = widgets
        .into_iter()
        .map(|mut w| {
            if let Some(cmd) = w.as_command_mut() {
                cmd.set_notify(widget_wake_tx.clone());
            }
            if let Some(clip) = w.as_clipboard_mut() {
                clip.set_notify(widget_wake_tx.clone());
            }
            if let Some(notif) = w.as_notifications_mut() {
                notif.set_notify(widget_wake_tx.clone());
            }
            w
        })
        .collect();

    state.widgets = widgets;

    // Setup layer surface
    state.setup_surface(&qh)?;

    // Setup Calloop event loop
    let mut event_loop: EventLoop<FlatbarState> =
        EventLoop::try_new().map_err(|e| format!("Failed to create Calloop event loop: {e}"))?;

    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .map_err(|e| format!("Failed to insert WaylandSource into Calloop: {e}"))?;

    // Register per-widget timer sources
    let loop_handle = event_loop.handle();
    for (wid, iv) in widget_intervals {
        let timer = Timer::from_duration(iv);
        let qh_timer = qh.clone();
        let widget_id = wid.clone();
        loop_handle
            .insert_source(timer, move |_event, _metadata, state: &mut FlatbarState| {
                if let Some(w) = state.widgets.iter().find(|w| w.id() == widget_id) {
                    w.refresh();
                }
                state.queue_redraw(&qh_timer);
                TimeoutAction::ToDuration(iv)
            })
            .map_err(|e| format!("Failed to register timer source for widget {wid}: {e}"))?;
    }

    // Register launcher channel source
    let (launcher_tx, launcher_rx) = calloop::channel::channel::<(String, String)>();
    state.launcher_tx = Some(launcher_tx);
    let qh_launcher = qh.clone();
    loop_handle
        .insert_source(
            launcher_rx,
            move |event, _metadata, state: &mut FlatbarState| {
                if let calloop::channel::Event::Msg((widget_id, action_id)) = event {
                    tracing::debug!(
                        "Launcher selected action '{action_id}' for widget '{widget_id}'"
                    );
                    if let Some(widget) = state.widgets.iter().find(|w| w.id() == widget_id) {
                        widget.handle_menu_action(&action_id);
                        state.damage_tracker.invalidate_all();
                        state.queue_redraw(&qh_launcher);
                    }
                }
            },
        )
        .map_err(|e| format!("Failed to register launcher channel source: {e}"))?;

    // Register widget wake channel: async command output arriving between
    // timer ticks triggers an immediate redraw.
    let qh_wake = qh.clone();
    loop_handle
        .insert_source(
            widget_wake_rx,
            move |_event, _metadata, state: &mut FlatbarState| {
                state.damage_tracker.invalidate_all();
                state.queue_redraw(&qh_wake);
            },
        )
        .map_err(|e| format!("Failed to register widget wake channel source: {e}"))?;

    // Setup DBus bridge for TrayWidget if present
    let has_tray = state.widgets.iter().any(|w| w.as_tray().is_some());

    if has_tray {
        let (tray_event_tx, tray_event_rx) =
            calloop::channel::channel::<crate::dbus::sni::TrayEvent>();
        let (std_tx, std_rx) = std::sync::mpsc::channel::<crate::dbus::sni::TrayEvent>();

        let tray_tx_clone = tray_event_tx.clone();
        std::thread::Builder::new()
            .name("flatbar-tray-forwarder".to_string())
            .spawn(move || {
                while let Ok(evt) = std_rx.recv() {
                    let _ = tray_tx_clone.send(evt);
                }
            })
            .ok();

        let fallback_poll_secs = state
            .widgets
            .iter()
            .find_map(|w| w.as_tray().map(|t| t.fallback_poll_secs()))
            .unwrap_or(10);

        if let Ok((handle, _join)) =
            crate::dbus::bridge::start_dbus_bridge(std_tx, fallback_poll_secs)
        {
            for w in &state.widgets {
                if let Some(tw) = w.as_tray() {
                    tw.set_dbus_handle(handle.clone());
                }
            }
            state.dbus_handle = Some(handle);

            let qh_tray = qh.clone();
            let _ = loop_handle.insert_source(
                tray_event_rx,
                move |event, _metadata, state: &mut FlatbarState| {
                    if let calloop::channel::Event::Msg(tray_evt) = event {
                        for w in &state.widgets {
                            if let Some(tw) = w.as_tray() {
                                tw.handle_event(tray_evt.clone());
                            }
                        }
                        state.damage_tracker.invalidate_all();
                        state.queue_redraw(&qh_tray);
                    }
                },
            );
        }
    }

    // Register SIGHUP listener for live config reload
    if let Ok(signals) = Signals::new(&[Signal::SIGHUP]) {
        let qh_sig = qh.clone();
        let path_opt = config_path.clone();
        if let Err(e) = loop_handle.insert_source(
            signals,
            move |_event, _metadata, state: &mut FlatbarState| {
                if let Some(ref path) = path_opt {
                    state.reload_config(path, &qh_sig);
                }
            },
        ) {
            tracing::warn!("Failed to register SIGHUP signal source: {e}");
        }
    }

    tracing::info!(
        "Flatbar event loop started for bar '{}'",
        state.config.bar.name
    );

    while !state.exit {
        event_loop
            .dispatch(Duration::from_millis(100), &mut state)
            .map_err(|e| format!("Calloop dispatch error: {e}"))?;
    }

    Ok(())
}

/// Parse the datetime strftime pattern menu config:
/// `patterns = [{ label = "Long date", format = "%A, %B %d, %Y" }, ...]`.
fn parse_datetime_patterns(val: &toml::Value) -> Vec<(String, String)> {
    val.get("patterns")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|pattern| {
                    let label = pattern.get("label")?.as_str()?.to_string();
                    let format = pattern.get("format")?.as_str()?.to_string();
                    Some((label, format))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Construct all widgets from configuration.
pub fn build_widgets(config: &Config) -> Vec<Box<dyn Widget>> {
    use crate::widget::battery::BatteryWidget;
    use crate::widget::bluetooth::{BluetoothConfig, BluetoothWidget};
    use crate::widget::clipboard::ClipboardWidget;
    use crate::widget::command::{CommandWidget, MouseActions};
    use crate::widget::datetime::DatetimeWidget;
    use crate::widget::disk::DiskWidget;
    use crate::widget::network::NetworkWidget;
    use crate::widget::notifications::NotificationsWidget;
    use crate::widget::stats::StatsWidget;
    use crate::widget::tray::{TrayConfig, TrayWidget};
    use crate::widget::wifi::WifiWidget;
    use crate::widget::workspaces::WorkspaceWidget;
    use std::collections::{HashMap, HashSet};

    let mut widgets: Vec<Box<dyn Widget>> = Vec::new();
    let mut configured_ids = HashSet::new();

    for (id, val) in &config.widgets {
        configured_ids.insert(id.clone());
        let widget_type =
            val.get("type")
                .and_then(|t| t.as_str())
                .unwrap_or(if val.get("command").is_some() {
                    "command"
                } else {
                    id.as_str()
                });

        let actions = MouseActions {
            left_click: val
                .get("left_click")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string()),
            right_click: val
                .get("right_click")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string()),
            middle_click: val
                .get("middle_click")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string()),
            scroll_up: val
                .get("scroll_up")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string()),
            scroll_down: val
                .get("scroll_down")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string()),
        };

        match widget_type {
            "datetime" => {
                let format = val
                    .get("format")
                    .and_then(|f| f.as_str())
                    .unwrap_or("%Y-%m-%d %H:%M:%S");
                let interval = val
                    .get("interval")
                    .and_then(|i| i.as_integer())
                    .unwrap_or(1) as u64;
                let calendar = val
                    .get("calendar")
                    .and_then(|c| c.as_bool())
                    .unwrap_or(true);
                let first_day = val
                    .get("first_day")
                    .and_then(|s| s.as_str())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();
                let calendar_launch =
                    val.get("calendar_launch")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                                .collect()
                        });
                let patterns = parse_datetime_patterns(val);

                widgets.push(Box::new(
                    DatetimeWidget::new(id, format, interval)
                        .with_actions(actions)
                        .with_calendar(calendar)
                        .with_first_day(first_day)
                        .with_calendar_launch(calendar_launch)
                        .with_patterns(patterns)
                        .with_window_rules(config.window_rules.clone()),
                ));
            }
            "stats" => {
                let interval = val
                    .get("interval")
                    .and_then(|i| i.as_integer())
                    .unwrap_or(2) as u64;
                let show_cpu = val.get("cpu").and_then(|v| v.as_bool()).unwrap_or(true);
                let show_ram = val.get("ram").and_then(|v| v.as_bool()).unwrap_or(true);
                let show_temp = val.get("temp").and_then(|v| v.as_bool()).unwrap_or(true);
                widgets.push(Box::new(
                    StatsWidget::new(id, interval, show_cpu, show_ram, show_temp)
                        .with_actions(actions)
                        .with_window_rules(config.window_rules.clone()),
                ));
            }
            "disk" => {
                let interval = val
                    .get("interval")
                    .and_then(|i| i.as_integer())
                    .unwrap_or(30) as u64;
                let mounts = val
                    .get("mounts")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|item| item.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["/".to_string()]);
                let warn_above = val.get("warn_above").and_then(|v| v.as_float());
                let units = val
                    .get("units")
                    .and_then(|v| v.as_str())
                    .map(crate::widget::disk::DiskUnits::parse)
                    .unwrap_or(crate::widget::disk::DiskUnits::Auto);
                let file_manager = val
                    .get("file_manager")
                    .and_then(|v| v.as_str())
                    .unwrap_or("rc")
                    .to_string();
                widgets.push(Box::new(
                    DiskWidget::new(id, mounts, interval, warn_above)
                        .with_actions(actions)
                        .with_units(units)
                        .with_file_manager(file_manager)
                        .with_window_rules(config.window_rules.clone()),
                ));
            }
            "battery" => {
                let interval = val
                    .get("interval")
                    .and_then(|i| i.as_integer())
                    .unwrap_or(5) as u64;
                let low_below = val
                    .get("low_below")
                    .and_then(|v| v.as_integer())
                    .map(|v| v as u8);
                let assume_ac = val
                    .get("assume_ac")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                widgets.push(Box::new(
                    BatteryWidget::new(id, interval, low_below)
                        .with_actions(actions)
                        .with_assume_ac(assume_ac),
                ));
            }
            "performance" => {
                let interval = val
                    .get("interval")
                    .and_then(|i| i.as_integer())
                    .unwrap_or(5) as u64;
                let icons = crate::widget::performance::PerformanceIcons {
                    powersaver: val
                        .get("icon_powersaver")
                        .and_then(|v| v.as_str())
                        .unwrap_or("fa-leaf")
                        .to_string(),
                    balanced: val
                        .get("icon_balanced")
                        .and_then(|v| v.as_str())
                        .unwrap_or("fa-scale-balanced")
                        .to_string(),
                    performance: val
                        .get("icon_performance")
                        .and_then(|v| v.as_str())
                        .unwrap_or("fa-rocket")
                        .to_string(),
                };
                widgets.push(Box::new(
                    crate::widget::performance::PerformanceWidget::new(id, interval)
                        .with_icons(icons),
                ));
            }
            "network" => {
                let interval = val
                    .get("interval")
                    .and_then(|i| i.as_integer())
                    .unwrap_or(3) as u64;
                let interface = val
                    .get("interface")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let show_device = val
                    .get("show_device")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let public_ip = val
                    .get("public_ip")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let public_ip_url = val
                    .get("public_ip_url")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                widgets.push(Box::new(
                    NetworkWidget::new(id, interval)
                        .with_actions(actions)
                        .with_interface(interface)
                        .with_show_device(show_device)
                        .with_public_ip(public_ip, public_ip_url)
                        .with_window_rules(config.window_rules.clone()),
                ));
            }
            "wifi" => {
                let interval = val
                    .get("interval")
                    .and_then(|i| i.as_integer())
                    .unwrap_or(5) as u64;
                let interface = val
                    .get("interface")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let show_device = val
                    .get("show_device")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                widgets.push(Box::new(
                    WifiWidget::new(id, interval)
                        .with_actions(actions)
                        .with_interface(interface)
                        .with_show_device(show_device)
                        .with_window_rules(config.window_rules.clone()),
                ));
            }
            "workspaces" => {
                widgets.push(Box::new(WorkspaceWidget::new(id)));
            }
            "bluetooth" => {
                let interval_secs = val
                    .get("interval")
                    .and_then(|i| i.as_integer())
                    .unwrap_or(5) as u64;
                let format = val
                    .get("format")
                    .and_then(|f| f.as_str())
                    .unwrap_or("{icon}")
                    .to_string();
                let icon_off = val
                    .get("icon_off")
                    .and_then(|ic| ic.as_str())
                    .unwrap_or("fa-bluetooth")
                    .to_string();
                let icon_on = val
                    .get("icon_on")
                    .and_then(|ic| ic.as_str())
                    .unwrap_or("fa-bluetooth")
                    .to_string();
                let icon_connected = val
                    .get("icon_connected")
                    .and_then(|ic| ic.as_str())
                    .unwrap_or("fa-bluetooth-b")
                    .to_string();
                let emphasis_on_connected = val
                    .get("emphasis_on_connected")
                    .and_then(|e| e.as_bool())
                    .unwrap_or(true);
                let menu_backend = val
                    .get("menu_backend")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        if s == "launcher" {
                            crate::config::MenuBackend::Launcher
                        } else {
                            crate::config::MenuBackend::Popup
                        }
                    })
                    .unwrap_or(config.bar.menu_backend);

                let manage_command = val
                    .get("manage_command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("bluetui")
                    .to_string();

                let bt_config = BluetoothConfig {
                    interval_secs,
                    format,
                    icon_off,
                    icon_on,
                    icon_connected,
                    emphasis_on_connected,
                    menu_backend,
                    manage_command,
                    window_rules: config.window_rules.clone(),
                };
                widgets.push(Box::new(BluetoothWidget::new(id, bt_config)));
            }
            "systray" | "tray" => {
                let summary_icon = val
                    .get("summary_icon")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fa-chevron-right")
                    .to_string();
                let attention_icon = val
                    .get("attention_icon")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fa-chevron-down")
                    .to_string();
                let hidden = val
                    .get("hidden")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|s| s.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let only = val
                    .get("only")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|s| s.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let menu_backend = val
                    .get("menu_backend")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        if s == "launcher" {
                            crate::config::MenuBackend::Launcher
                        } else {
                            crate::config::MenuBackend::Popup
                        }
                    })
                    .unwrap_or(config.bar.menu_backend);
                let icon_size = val
                    .get("icon_size")
                    .and_then(|v| v.as_integer())
                    .unwrap_or(16) as u32;
                let fallback_poll_secs = val
                    .get("tray_poll_fallback_secs")
                    .and_then(|v| v.as_integer())
                    .map(|v| v.max(0) as u64)
                    .unwrap_or(10);

                let tray_config = TrayConfig {
                    summary_icon,
                    attention_icon,
                    hidden,
                    only,
                    menu_backend,
                    icon_size,
                    fallback_poll_secs,
                };
                widgets.push(Box::new(TrayWidget::new(id, tray_config)));
            }
            "command" => {
                let cmd = val.get("command").and_then(|c| c.as_str()).unwrap_or("");
                let interval = val
                    .get("interval")
                    .and_then(|i| i.as_integer())
                    .unwrap_or(0) as u64;
                let signal = val
                    .get("signal")
                    .and_then(|s| s.as_integer())
                    .map(|s| s as u8);
                let default_icon = val
                    .get("icon")
                    .and_then(|ic| ic.as_str())
                    .map(|s| s.to_string());

                let mut class_icons = HashMap::new();
                if let Some(icons_tbl) = val.get("icons").and_then(|ic| ic.as_table()) {
                    for (k, v) in icons_tbl {
                        if let Some(ic_str) = v.as_str() {
                            class_icons.insert(k.clone(), ic_str.to_string());
                        }
                    }
                }

                let actions = MouseActions {
                    left_click: val
                        .get("left_click")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string()),
                    right_click: val
                        .get("right_click")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string()),
                    middle_click: val
                        .get("middle_click")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string()),
                    scroll_up: val
                        .get("scroll_up")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string()),
                    scroll_down: val
                        .get("scroll_down")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string()),
                };

                let volume_icons = crate::widget::command::VolumeThresholdIcons::from_config(val);
                let mute_command = val
                    .get("mute_command")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                widgets.push(Box::new(CommandWidget::with_volume(
                    id,
                    cmd,
                    interval,
                    signal,
                    actions,
                    default_icon,
                    class_icons,
                    volume_icons,
                    mute_command,
                )));
            }
            _ if val.get("command").is_some() => {
                let cmd = val.get("command").and_then(|c| c.as_str()).unwrap_or("");
                let interval = val
                    .get("interval")
                    .and_then(|i| i.as_integer())
                    .unwrap_or(0) as u64;
                let signal = val
                    .get("signal")
                    .and_then(|s| s.as_integer())
                    .map(|s| s as u8);
                let default_icon = val
                    .get("icon")
                    .and_then(|ic| ic.as_str())
                    .map(|s| s.to_string());

                let mut class_icons = HashMap::new();
                if let Some(icons_tbl) = val.get("icons").and_then(|ic| ic.as_table()) {
                    for (k, v) in icons_tbl {
                        if let Some(ic_str) = v.as_str() {
                            class_icons.insert(k.clone(), ic_str.to_string());
                        }
                    }
                }

                let actions = MouseActions {
                    left_click: val
                        .get("left_click")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string()),
                    right_click: val
                        .get("right_click")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string()),
                    middle_click: val
                        .get("middle_click")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string()),
                    scroll_up: val
                        .get("scroll_up")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string()),
                    scroll_down: val
                        .get("scroll_down")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string()),
                };

                widgets.push(Box::new(CommandWidget::new(
                    id,
                    cmd,
                    interval,
                    signal,
                    actions,
                    default_icon,
                    class_icons,
                )));
            }
            "clipboard" => {
                let icon = val
                    .get("icon")
                    .and_then(|ic| ic.as_str())
                    .unwrap_or(crate::widget::clipboard::DEFAULT_CLIPBOARD_ICON)
                    .to_string();
                widgets.push(Box::new(
                    ClipboardWidget::new(id)
                        .with_actions(actions)
                        .with_icon(icon),
                ));
            }
            "notifications" => {
                widgets.push(Box::new(NotificationsWidget::new(id).with_actions(actions)));
            }
            _ => {
                tracing::warn!("Unknown widget type for widget '{}'", id);
            }
        }
    }

    // Also instantiate any layout widgets that were referenced but not explicitly configured in [widgets.<id>]
    let all_layout_ids = config
        .bar
        .layout
        .left
        .iter()
        .chain(config.bar.layout.center.iter())
        .chain(config.bar.layout.right.iter());

    for id in all_layout_ids {
        if !configured_ids.contains(id) {
            match id.as_str() {
                "datetime" => {
                    widgets.push(Box::new(DatetimeWidget::new(
                        "datetime",
                        "%Y-%m-%d %H:%M:%S",
                        1,
                    )));
                    configured_ids.insert("datetime".to_string());
                }
                "workspaces" => {
                    widgets.push(Box::new(WorkspaceWidget::new("workspaces")));
                    configured_ids.insert("workspaces".to_string());
                }
                "stats" => {
                    widgets.push(Box::new(StatsWidget::new("stats", 2, true, true, true)));
                    configured_ids.insert("stats".to_string());
                }
                "disk" => {
                    widgets.push(Box::new(DiskWidget::new(
                        "disk",
                        vec!["/".to_string()],
                        30,
                        None,
                    )));
                    configured_ids.insert("disk".to_string());
                }
                "battery" => {
                    widgets.push(Box::new(BatteryWidget::new("battery", 5, None)));
                    configured_ids.insert("battery".to_string());
                }
                "performance" => {
                    widgets.push(Box::new(
                        crate::widget::performance::PerformanceWidget::new("performance", 5),
                    ));
                    configured_ids.insert("performance".to_string());
                }
                "network" => {
                    widgets.push(Box::new(NetworkWidget::new("network", 3)));
                    configured_ids.insert("network".to_string());
                }
                "wifi" => {
                    widgets.push(Box::new(WifiWidget::new("wifi", 5)));
                    configured_ids.insert("wifi".to_string());
                }
                "bluetooth" => {
                    let bt_config = BluetoothConfig {
                        menu_backend: config.bar.menu_backend,
                        window_rules: config.window_rules.clone(),
                        ..Default::default()
                    };
                    widgets.push(Box::new(BluetoothWidget::new(id.as_str(), bt_config)));
                    configured_ids.insert(id.clone());
                }
                "systray" | "tray" => {
                    let tray_config = TrayConfig {
                        menu_backend: config.bar.menu_backend,
                        ..Default::default()
                    };
                    widgets.push(Box::new(TrayWidget::new(id.as_str(), tray_config)));
                    configured_ids.insert(id.clone());
                }
                "clipboard" => {
                    widgets.push(Box::new(
                        ClipboardWidget::new(id.as_str())
                            .with_icon(crate::widget::clipboard::DEFAULT_CLIPBOARD_ICON),
                    ));
                    configured_ids.insert(id.clone());
                }
                "notifications" => {
                    widgets.push(Box::new(NotificationsWidget::new(id.as_str())));
                    configured_ids.insert(id.clone());
                }
                _ => {}
            }
        }
    }

    widgets
}
