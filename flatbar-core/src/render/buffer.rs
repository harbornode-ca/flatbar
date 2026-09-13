//! Double-buffered wl_shm SlotPool manager for drawing frames.

use crate::config::Color;
use smithay_client_toolkit::shm::slot::{Buffer as SlotBuffer, SlotPool};
use smithay_client_toolkit::shm::Shm;
use wayland_client::protocol::wl_shm::Format;

/// Surface drawing buffer manager wrapping SlotPool.
pub struct BufferManager {
    pool: SlotPool,
    width: u32,
    height: u32,
}

impl BufferManager {
    pub fn new(shm: &Shm, width: u32, height: u32) -> Result<Self, String> {
        let physical_bytes = (width as usize) * (height as usize) * 4 * 2; // 2 buffers
        let pool = SlotPool::new(physical_bytes.max(4096), shm)
            .map_err(|e| format!("Failed to create SlotPool: {e}"))?;

        Ok(Self {
            pool,
            width,
            height,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// Allocate a buffer slot and return the buffer and canvas.
    pub fn create_buffer(&mut self) -> Result<(SlotBuffer, &mut [u8]), String> {
        let stride = (self.width * 4) as i32;
        self.pool
            .create_buffer(
                self.width as i32,
                self.height as i32,
                stride,
                Format::Argb8888,
            )
            .map_err(|e| format!("Failed to allocate buffer slot: {e}"))
    }

    /// Clear the raw byte canvas to a solid background color.
    pub fn clear_canvas(canvas: &mut [u8], color: Color) {
        let pixel = color.to_argb_u32();
        let bytes = pixel.to_ne_bytes();

        for chunk in canvas.chunks_exact_mut(4) {
            chunk.copy_from_slice(&bytes);
        }
    }
}
