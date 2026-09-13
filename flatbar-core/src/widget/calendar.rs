//! Calendar model, layout computation, and frame buffer rendering for the Datetime popup.

use std::str::FromStr;

use crate::config::Color;
use crate::render::layout::Rect;
use crate::render::text::{TextRenderParams, TextRenderer};
use crate::shell::scaling::logical_rect_to_buffer;

/// First day of the week setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FirstDay {
    #[default]
    Monday,
    Sunday,
}

impl FromStr for FirstDay {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "sunday" | "sun" => Ok(FirstDay::Sunday),
            _ => Ok(FirstDay::Monday),
        }
    }
}

/// Check if a year is a leap year in Gregorian calendar.
pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Return the number of days in the specified month (1..=12).
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Calculate the day of the week for day 1 of given year/month.
/// Returns 0 for Sunday, 1 for Monday, ..., 6 for Saturday.
pub fn day_of_week_1st(year: i32, month: u32) -> u32 {
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    let m = (month.clamp(1, 12) - 1) as usize;
    let d = 1;
    let dow = (y + y / 4 - y / 100 + y / 400 + t[m] + d) % 7;
    ((dow % 7 + 7) % 7) as u32
}

/// Calculate the starting column index (0..7) for month's 1st day.
pub fn month_start_offset(year: i32, month: u32, first_day: FirstDay) -> u32 {
    let dow = day_of_week_1st(year, month);
    match first_day {
        FirstDay::Sunday => dow,
        FirstDay::Monday => (dow + 6) % 7,
    }
}

/// Return English month name.
pub fn month_name(month: u32) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "",
    }
}

/// Query system for current (year, month, day).
pub fn get_current_date() -> (i32, u32, u32) {
    unsafe {
        let mut now: libc::time_t = 0;
        libc::time(&mut now);
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        (tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32)
    }
}

/// Pure calendar state model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarModel {
    pub year: i32,
    pub month: u32,
    pub first_day: FirstDay,
    pub today: (i32, u32, u32),
}

impl CalendarModel {
    pub fn new(year: i32, month: u32, first_day: FirstDay, today: (i32, u32, u32)) -> Self {
        Self {
            year,
            month: month.clamp(1, 12),
            first_day,
            today,
        }
    }

    pub fn current(first_day: FirstDay) -> Self {
        let today = get_current_date();
        Self {
            year: today.0,
            month: today.1,
            first_day,
            today,
        }
    }

    pub fn prev_month(&mut self) {
        if self.month == 1 {
            self.month = 12;
            self.year -= 1;
        } else {
            self.month -= 1;
        }
    }

    pub fn next_month(&mut self) {
        if self.month == 12 {
            self.month = 1;
            self.year += 1;
        } else {
            self.month += 1;
        }
    }
}

/// A single day cell in the calendar layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarDayCell {
    pub day: u32,
    pub is_today: bool,
    pub rect: Rect,
}

/// Computed geometry layout for calendar popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarLayout {
    pub width: u32,
    pub height: u32,
    pub prev_btn_rect: Rect,
    pub next_btn_rect: Rect,
    pub title_rect: Rect,
    pub day_headers: Vec<(String, Rect)>,
    pub cells: Vec<CalendarDayCell>,
    /// Optional launch button rendered under the calendar grid.
    pub launcher: Option<(String, Rect)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarHit {
    PrevMonth,
    NextMonth,
    Day(u32),
    Launcher,
    None,
}

impl CalendarLayout {
    pub fn hit_test(&self, x: i32, y: i32) -> CalendarHit {
        if self.prev_btn_rect.contains(x, y) {
            return CalendarHit::PrevMonth;
        }
        if self.next_btn_rect.contains(x, y) {
            return CalendarHit::NextMonth;
        }
        if let Some((_, rect)) = &self.launcher {
            if rect.contains(x, y) {
                return CalendarHit::Launcher;
            }
        }
        if let Some(cell) = self.cells.iter().find(|c| c.rect.contains(x, y)) {
            CalendarHit::Day(cell.day)
        } else {
            CalendarHit::None
        }
    }
}

/// Compute calendar popup geometry. When `launch_command` is set, a full-width
/// button labelled with the command name is drawn under the day grid.
pub fn layout_calendar(
    model: &CalendarModel,
    _text_renderer: &TextRenderer,
    _font_family: &str,
    font_size: f32,
    launch_command: Option<&[String]>,
) -> CalendarLayout {
    let padding: i32 = 12;
    let cell_w: i32 = (font_size * 2.2).round().max(28.0) as i32;
    let cell_h: i32 = (font_size * 1.8).round().max(24.0) as i32;
    let total_w = (padding * 2 + cell_w * 7) as u32;

    let header_h: i32 = 30;
    let day_name_h: i32 = 22;

    let prev_btn_rect = Rect::new(padding, 6, 24, (header_h - 6).max(0) as u32);
    let next_btn_rect = Rect::new(
        total_w as i32 - padding - 24,
        6,
        24,
        (header_h - 6).max(0) as u32,
    );
    let title_w = (total_w as i32 - (padding + 28) * 2).max(0) as u32;
    let title_rect = Rect::new(padding + 28, 6, title_w, (header_h - 6).max(0) as u32);

    let day_labels = match model.first_day {
        FirstDay::Monday => ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"],
        FirstDay::Sunday => ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"],
    };

    let mut day_headers = Vec::new();
    let header_y: i32 = header_h + 4;
    for (col, label) in day_labels.iter().enumerate() {
        let x = padding + col as i32 * cell_w;
        day_headers.push((
            label.to_string(),
            Rect::new(x, header_y, cell_w as u32, day_name_h as u32),
        ));
    }

    let start_offset = month_start_offset(model.year, model.month, model.first_day);
    let num_days = days_in_month(model.year, model.month);

    let mut cells = Vec::new();
    let grid_start_y: i32 = header_y + day_name_h + 4;

    for day in 1..=num_days {
        let slot = start_offset + day - 1;
        let col = slot % 7;
        let row = slot / 7;

        let x = padding + col as i32 * cell_w;
        let y = grid_start_y + row as i32 * cell_h;

        let is_today =
            model.today.0 == model.year && model.today.1 == model.month && model.today.2 == day;

        cells.push(CalendarDayCell {
            day,
            is_today,
            rect: Rect::new(x, y, cell_w as u32, cell_h as u32),
        });
    }

    let num_rows = (start_offset + num_days).div_ceil(7).max(5) as i32;
    let mut total_h = (grid_start_y + num_rows * cell_h + padding).max(0) as u32;

    // Optional launch button under the grid.
    let launcher = launch_command
        .and_then(|cmd| cmd.first().cloned())
        .map(|label| {
            let btn_y = grid_start_y + num_rows * cell_h + 4;
            let btn_h: i32 = 28;
            total_h = (btn_y + btn_h + padding).max(0) as u32;
            (
                format!("▶ {label}"),
                Rect::new(
                    padding + 4,
                    btn_y,
                    (total_w as i32 - (padding + 8)).max(0) as u32,
                    btn_h as u32,
                ),
            )
        });

    CalendarLayout {
        width: total_w,
        height: total_h,
        prev_btn_rect,
        next_btn_rect,
        title_rect,
        day_headers,
        cells,
        launcher,
    }
}

/// Parameters for rendering the calendar popup surface.
pub struct CalendarRenderParams<'a> {
    pub model: &'a CalendarModel,
    pub layout: &'a CalendarLayout,
    pub hovered: CalendarHit,
    pub font_family: &'a str,
    pub font_size: f32,
    pub scale: f64,
    pub bg_color: Color,
    pub fg_color: Color,
}

/// Render the calendar popup buffer.
pub fn render_calendar(
    pixels: &mut [u32],
    buffer_width: u32,
    buffer_height: u32,
    text_renderer: &TextRenderer,
    params: &CalendarRenderParams<'_>,
) {
    let bg_u32 = params.bg_color.to_argb_u32();
    let fg_u32 = params.fg_color.to_argb_u32();

    for p in pixels.iter_mut() {
        *p = bg_u32;
    }

    // Border
    for x in 0..buffer_width {
        pixels[x as usize] = fg_u32;
        let b = ((buffer_height - 1) * buffer_width + x) as usize;
        if b < pixels.len() {
            pixels[b] = fg_u32;
        }
    }
    for y in 0..buffer_height {
        pixels[(y * buffer_width) as usize] = fg_u32;
        let r = (y * buffer_width + (buffer_width - 1)) as usize;
        if r < pixels.len() {
            pixels[r] = fg_u32;
        }
    }

    // Header buttons `<` and `>`
    let prev_hover = params.hovered == CalendarHit::PrevMonth;
    let next_hover = params.hovered == CalendarHit::NextMonth;

    let (px, py, pw, ph) = logical_rect_to_buffer(
        params.layout.prev_btn_rect.x,
        params.layout.prev_btn_rect.y,
        params.layout.prev_btn_rect.width,
        params.layout.prev_btn_rect.height,
        params.scale,
    );
    if prev_hover {
        for dy in 0..ph {
            for dx in 0..pw {
                let idx =
                    ((py + dy as i32) as u32 * buffer_width + (px + dx as i32) as u32) as usize;
                if idx < pixels.len() {
                    pixels[idx] = fg_u32;
                }
            }
        }
    }
    let prev_fg = if prev_hover {
        params.bg_color
    } else {
        params.fg_color
    };
    let t_prev = TextRenderParams {
        text: "<",
        font_family: params.font_family,
        font_size_pt: params.font_size,
        scale: params.scale,
        dest_x: px + (pw as i32 / 3),
        dest_y: py + 2,
        fg_color: prev_fg,
    };
    text_renderer.render_text(pixels, buffer_width, buffer_height, &t_prev);

    let (nx, ny, nw, nh) = logical_rect_to_buffer(
        params.layout.next_btn_rect.x,
        params.layout.next_btn_rect.y,
        params.layout.next_btn_rect.width,
        params.layout.next_btn_rect.height,
        params.scale,
    );
    if next_hover {
        for dy in 0..nh {
            for dx in 0..nw {
                let idx =
                    ((ny + dy as i32) as u32 * buffer_width + (nx + dx as i32) as u32) as usize;
                if idx < pixels.len() {
                    pixels[idx] = fg_u32;
                }
            }
        }
    }
    let next_fg = if next_hover {
        params.bg_color
    } else {
        params.fg_color
    };
    let t_next = TextRenderParams {
        text: ">",
        font_family: params.font_family,
        font_size_pt: params.font_size,
        scale: params.scale,
        dest_x: nx + (nw as i32 / 3),
        dest_y: ny + 2,
        fg_color: next_fg,
    };
    text_renderer.render_text(pixels, buffer_width, buffer_height, &t_next);

    // Title: Month Year
    let title_text = format!("{} {}", month_name(params.model.month), params.model.year);
    let (tx, ty, tw, _) = logical_rect_to_buffer(
        params.layout.title_rect.x,
        params.layout.title_rect.y,
        params.layout.title_rect.width,
        params.layout.title_rect.height,
        params.scale,
    );
    let (measured_w, _) = text_renderer.measure_text(&TextRenderParams {
        text: &title_text,
        font_family: params.font_family,
        font_size_pt: params.font_size,
        scale: params.scale,
        dest_x: 0,
        dest_y: 0,
        fg_color: params.fg_color,
    });
    let title_x = tx + ((tw as i32 - measured_w as i32) / 2).max(0);
    text_renderer.render_text(
        pixels,
        buffer_width,
        buffer_height,
        &TextRenderParams {
            text: &title_text,
            font_family: params.font_family,
            font_size_pt: params.font_size,
            scale: params.scale,
            dest_x: title_x,
            dest_y: ty + 2,
            fg_color: params.fg_color,
        },
    );

    // Day headers (Mo Tu We ...)
    for (name, rect) in &params.layout.day_headers {
        let (hx, hy, hw, _) =
            logical_rect_to_buffer(rect.x, rect.y, rect.width, rect.height, params.scale);
        let (mw, _) = text_renderer.measure_text(&TextRenderParams {
            text: name,
            font_family: params.font_family,
            font_size_pt: params.font_size * 0.9,
            scale: params.scale,
            dest_x: 0,
            dest_y: 0,
            fg_color: params.fg_color,
        });
        let cx = hx + ((hw as i32 - mw as i32) / 2).max(0);
        text_renderer.render_text(
            pixels,
            buffer_width,
            buffer_height,
            &TextRenderParams {
                text: name,
                font_family: params.font_family,
                font_size_pt: params.font_size * 0.9,
                scale: params.scale,
                dest_x: cx,
                dest_y: hy,
                fg_color: params.fg_color,
            },
        );
    }

    // Day cells
    for cell in &params.layout.cells {
        let (cx, cy, cw, ch) = logical_rect_to_buffer(
            cell.rect.x,
            cell.rect.y,
            cell.rect.width,
            cell.rect.height,
            params.scale,
        );
        let is_hover = params.hovered == CalendarHit::Day(cell.day);

        // If today: inverted emphasis filled box
        if cell.is_today {
            for dy in 1..(ch.saturating_sub(1)) {
                for dx in 1..(cw.saturating_sub(1)) {
                    let idx =
                        ((cy + dy as i32) as u32 * buffer_width + (cx + dx as i32) as u32) as usize;
                    if idx < pixels.len() {
                        pixels[idx] = fg_u32;
                    }
                }
            }
        } else if is_hover {
            // Hover outline
            for dx in 1..(cw.saturating_sub(1)) {
                let t_idx = ((cy + 1) as u32 * buffer_width + (cx + dx as i32) as u32) as usize;
                let b_idx =
                    ((cy + ch as i32 - 2) as u32 * buffer_width + (cx + dx as i32) as u32) as usize;
                if t_idx < pixels.len() {
                    pixels[t_idx] = fg_u32;
                }
                if b_idx < pixels.len() {
                    pixels[b_idx] = fg_u32;
                }
            }
            for dy in 1..(ch.saturating_sub(1)) {
                let l_idx = ((cy + dy as i32) as u32 * buffer_width + (cx + 1) as u32) as usize;
                let r_idx =
                    ((cy + dy as i32) as u32 * buffer_width + (cx + cw as i32 - 2) as u32) as usize;
                if l_idx < pixels.len() {
                    pixels[l_idx] = fg_u32;
                }
                if r_idx < pixels.len() {
                    pixels[r_idx] = fg_u32;
                }
            }
        }

        let day_str = cell.day.to_string();
        let (mw, _) = text_renderer.measure_text(&TextRenderParams {
            text: &day_str,
            font_family: params.font_family,
            font_size_pt: params.font_size,
            scale: params.scale,
            dest_x: 0,
            dest_y: 0,
            fg_color: params.fg_color,
        });

        let text_fg = if cell.is_today {
            params.bg_color
        } else {
            params.fg_color
        };
        let tx = cx + ((cw as i32 - mw as i32) / 2).max(0);
        let ty = cy + ((ch as i32 - (params.font_size * params.scale as f32) as i32) / 2).max(0);

        text_renderer.render_text(
            pixels,
            buffer_width,
            buffer_height,
            &TextRenderParams {
                text: &day_str,
                font_family: params.font_family,
                font_size_pt: params.font_size,
                scale: params.scale,
                dest_x: tx,
                dest_y: ty,
                fg_color: text_fg,
            },
        );
    }
    // Launch button (if present)
    if let Some((label, rect)) = &params.layout.launcher {
        let (bx, by, bw, bh) =
            logical_rect_to_buffer(rect.x, rect.y, rect.width, rect.height, params.scale);
        let is_hover = params.hovered == CalendarHit::Launcher;

        if is_hover {
            for dy in 0..bh {
                for dx in 0..bw {
                    let idx =
                        ((by + dy as i32) as u32 * buffer_width + (bx + dx as i32) as u32) as usize;
                    if idx < pixels.len() {
                        pixels[idx] = fg_u32;
                    }
                }
            }
        }

        let btn_fg = if is_hover {
            params.bg_color
        } else {
            params.fg_color
        };

        // 1px outline
        for dx in 0..bw {
            let t = (by as u32 * buffer_width + (bx + dx as i32) as u32) as usize;
            let b = ((by + bh as i32 - 1) as u32 * buffer_width + (bx + dx as i32) as u32) as usize;
            if t < pixels.len() {
                pixels[t] = fg_u32;
            }
            if b < pixels.len() {
                pixels[b] = fg_u32;
            }
        }
        for dy in 0..bh {
            let l = ((by + dy as i32) as u32 * buffer_width + bx as u32) as usize;
            let r = ((by + dy as i32) as u32 * buffer_width + (bx + bw as i32 - 1) as u32) as usize;
            if l < pixels.len() {
                pixels[l] = fg_u32;
            }
            if r < pixels.len() {
                pixels[r] = fg_u32;
            }
        }

        let (mw, _) = text_renderer.measure_text(&TextRenderParams {
            text: label,
            font_family: params.font_family,
            font_size_pt: params.font_size,
            scale: params.scale,
            dest_x: 0,
            dest_y: 0,
            fg_color: params.fg_color,
        });
        let label_x = bx + ((bw as i32 - mw as i32) / 2).max(0);
        let label_y =
            by + ((bh as i32 - (params.font_size * params.scale as f32) as i32) / 2).max(0);
        text_renderer.render_text(
            pixels,
            buffer_width,
            buffer_height,
            &TextRenderParams {
                text: label,
                font_family: params.font_family,
                font_size_pt: params.font_size,
                scale: params.scale,
                dest_x: label_x,
                dest_y: label_y,
                fg_color: btn_fg,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calendar_layout_with_launch_button() {
        let cal = CalendarModel::new(2026, 9, FirstDay::Monday, (2026, 9, 12));
        let renderer = TextRenderer::new();

        // No launcher configured: no button rect.
        let layout = layout_calendar(&cal, &renderer, "sans-serif", 13.0, None);
        assert!(layout.launcher.is_none());

        // Launcher configured: button under the grid, hit-testable.
        let cmd = vec!["gsimplecal".to_string()];
        let layout = layout_calendar(&cal, &renderer, "sans-serif", 13.0, Some(&cmd));
        let (_, rect) = layout.launcher.clone().unwrap();
        assert!(rect.height >= 20);
        let hit = layout.hit_test(rect.x + 5, rect.y + 5);
        assert_eq!(hit, CalendarHit::Launcher);
    }

    #[test]
    fn test_calendar_days_and_leap_years() {
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2025));
        assert!(!is_leap_year(1900));
        assert!(is_leap_year(2000));

        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2025, 2), 28);
        assert_eq!(days_in_month(2026, 9), 30);
        assert_eq!(days_in_month(2026, 8), 31);
    }

    #[test]
    fn test_calendar_first_day_offsets() {
        // Sept 1, 2026 is Tuesday.
        // Dow is 2 (0=Sun, 1=Mon, 2=Tue)
        assert_eq!(day_of_week_1st(2026, 9), 2);

        // Monday first -> Tuesday is offset 1
        assert_eq!(month_start_offset(2026, 9, FirstDay::Monday), 1);
        // Sunday first -> Tuesday is offset 2
        assert_eq!(month_start_offset(2026, 9, FirstDay::Sunday), 2);
    }

    #[test]
    fn test_calendar_navigation() {
        let mut cal = CalendarModel::new(2026, 9, FirstDay::Monday, (2026, 9, 12));
        assert_eq!(cal.year, 2026);
        assert_eq!(cal.month, 9);

        cal.next_month();
        assert_eq!(cal.month, 10);
        assert_eq!(cal.year, 2026);

        cal.prev_month();
        assert_eq!(cal.month, 9);

        // Wrapping test
        let mut cal_dec = CalendarModel::new(2026, 12, FirstDay::Monday, (2026, 12, 1));
        cal_dec.next_month();
        assert_eq!(cal_dec.year, 2027);
        assert_eq!(cal_dec.month, 1);

        cal_dec.prev_month();
        assert_eq!(cal_dec.year, 2026);
        assert_eq!(cal_dec.month, 12);
    }

    #[test]
    fn test_calendar_layout_and_hit_test() {
        let cal = CalendarModel::new(2026, 9, FirstDay::Monday, (2026, 9, 12));
        let renderer = TextRenderer::new();
        let layout = layout_calendar(&cal, &renderer, "sans-serif", 13.0, None);

        assert!(layout.width >= 180);
        assert!(layout.height >= 180);
        assert_eq!(layout.cells.len(), 30); // Sept has 30 days

        // Sept 12 should be marked today
        let cell12 = layout.cells.iter().find(|c| c.day == 12).unwrap();
        assert!(cell12.is_today);

        // Hit test prev button
        let hit_prev = layout.hit_test(layout.prev_btn_rect.x + 2, layout.prev_btn_rect.y + 2);
        assert_eq!(hit_prev, CalendarHit::PrevMonth);

        // Hit test next button
        let hit_next = layout.hit_test(layout.next_btn_rect.x + 2, layout.next_btn_rect.y + 2);
        assert_eq!(hit_next, CalendarHit::NextMonth);

        // Hit test day 12
        let hit_day = layout.hit_test(cell12.rect.x + 5, cell12.rect.y + 5);
        assert_eq!(hit_day, CalendarHit::Day(12));
    }
}
