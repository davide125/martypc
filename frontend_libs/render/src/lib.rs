/*
    MartyPC
    https://github.com/dbalsom/martypc

    Copyright 2022-2023 Daniel Balsom

    Permission is hereby granted, free of charge, to any person obtaining a
    copy of this software and associated documentation files (the “Software”),
    to deal in the Software without restriction, including without limitation
    the rights to use, copy, modify, merge, publish, distribute, sublicense,
    and/or sell copies of the Software, and to permit persons to whom the
    Software is furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in
    all copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER   
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
    FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
    DEALINGS IN THE SOFTWARE.

    ---------------------------------------------------------------------------

    render::mod.rs

    This module implements various Video rendering functions. Video card devices
    render in either Direct or Indirect mode.

    In direct mode, the video device draws directly to intermediate representation
    framebuffer, which the render module displays.

    In indirect mode, the render module draws the video device's VRAM directly. 
    This is fast, but not always accurate if register writes happen mid-frame.

*/

#![allow(dead_code)]
#![allow(clippy::identity_op)] // Adding 0 lines things up nicely for formatting.

use std::path::Path;

use bytemuck::*;

pub mod resize;
pub mod composite;

// Re-export submodules
pub use self::resize::*;
pub use self::composite::*;

use marty_core::{
    config::VideoType,
    videocard::{VideoCard, CGAColor, CGAPalette, CursorInfo, DisplayExtents, DisplayMode, FontInfo},
    devices::cga,
    bus::BusInterface,
    file_util
};

use image;
use log;

pub const ATTR_BLUE_FG: u8      = 0b0000_0001;
pub const ATTR_GREEN_FG: u8     = 0b0000_0010;
pub const ATTR_RED_FG: u8       = 0b0000_0100;
pub const ATTR_BRIGHT_FG: u8    = 0b0000_1000;
pub const ATTR_BLUE_BG: u8      = 0b0001_0000;
pub const ATTR_GREEN_BG: u8     = 0b0010_0000;
pub const ATTR_RED_BG: u8       = 0b0100_0000;
pub const ATTR_BRIGHT_BG: u8    = 0b1000_0000;

// Font is encoded as a bit pattern with a span of 256 bits per row
//static CGA_FONT: &'static [u8; 2048] = include_bytes!("cga_font.bin");

const CGA_FIELD_OFFSET: u32 = 8192;

const FONT_SPAN: u32 = 32;
//const FONT_W: u32 = 8;
//const FONT_H: u32 = 8;

const CGA_HIRES_GFX_W: u32 = 640;
const CGA_HIRES_GFX_H: u32 = 200;
const CGA_GFX_W: u32 = 320;
const CGA_GFX_H: u32 = 200;

const EGA_LORES_GFX_W: u32 = 320;
const EGA_LORES_GFX_H: u32 = 200;
const EGA_HIRES_GFX_W: u32 = 640;
const EGA_HIRES_GFX_H: u32 = 350;

const VGA_LORES_GFX_W: u32 = 320;
const VGA_LORES_GFX_H: u32 = 200;
const VGA_HIRES_GFX_W: u32 = 640;
const VGA_HIRES_GFX_H: u32 = 480;

const XOR_COLOR: u8 = 0x80;

#[derive (Copy, Clone, Default)]
pub struct VideoData {
    pub render_w: u32,
    pub render_h: u32,
    pub aspect_w: u32,
    pub aspect_h: u32,
    pub aspect_correction_enabled: bool,
    pub composite_params: CompositeParams
}


#[derive (Copy, Clone)]
pub struct AspectRatio {
    pub h: u32,
    pub v: u32,
}

#[derive (Copy, Clone)]
pub struct CompositeParams {
    pub hue: f32,
    pub sat: f32,
    pub luma: f32
}

impl Default for CompositeParams {
    fn default() -> Self {
        Self {
            hue: 1.0,
            sat: 1.15,
            luma: 1.15
        }
    }
}

#[derive (Copy, Clone)]
pub enum RenderColor {
    CgaIndex(u8),
    Rgb(u8, u8, u8)
}

#[derive (Copy, Clone)]
pub struct DebugRenderParams {
    pub draw_scanline: Option<u32>,
    pub draw_scanline_color: Option<RenderColor>
}

//const frame_w: u32 = 640;
//const frame_h: u32 = 400;

// This color-index to RGBA table supports two conversion palettes,
// the "standard" palette given by most online references, and the 
// alternate, more monitor-accurate "VileR palette"
// See https://int10h.org/blog/2022/06/ibm-5153-color-true-cga-palette/ 
// for details.
const CGA_RGBA_COLORS: &[[[u8; 4]; 16]; 2] = &[
    [
        [0x10, 0x10, 0x10, 0xFF], // 0 - Black  (Slightly brighter for debugging)
        [0x00, 0x00, 0xAA, 0xFF], // 1 - Blue
        [0x00, 0xAA, 0x00, 0xFF], // 2 - Green
        [0x00, 0xAA, 0xAA, 0xFF], // 3 - Cyan
        [0xAA, 0x00, 0x00, 0xFF], // 4 - Red
        [0xAA, 0x00, 0xAA, 0xFF], // 5 - Magenta
        [0xAA, 0x55, 0x00, 0xFF], // 6 - Brown
        [0xAA, 0xAA, 0xAA, 0xFF], // 7 - Light Gray
        [0x55, 0x55, 0x55, 0xFF], // 8 - Dark Gray
        [0x55, 0x55, 0xFF, 0xFF], // 9 - Light Blue
        [0x55, 0xFF, 0x55, 0xFF], // 10 - Light Green
        [0x55, 0xFF, 0xFF, 0xFF], // 11 - Light Cyan
        [0xFF, 0x55, 0x55, 0xFF], // 12 - Light Red
        [0xFF, 0x55, 0xFF, 0xFF], // 13 - Light Magenta
        [0xFF, 0xFF, 0x55, 0xFF], // 14 - Yellow
        [0xFF, 0xFF, 0xFF, 0xFF], // 15 - White
    ],
    // VileR's palette
    [
        [0x00, 0x00, 0x00, 0xFF], // 0 - Black
        [0x00, 0x00, 0xC4, 0xFF], // 1 - Blue
        [0x00, 0xC4, 0x00, 0xFF], // 2 - Green
        [0x00, 0xC4, 0xC4, 0xFF], // 3 - Cyan
        [0xC4, 0x00, 0x00, 0xFF], // 4 - Red
        [0xC4, 0x00, 0xC4, 0xFF], // 5 - Magenta
        [0xC4, 0x7E, 0x00, 0xFF], // 6 - Brown
        [0xC4, 0xC4, 0xC4, 0xFF], // 7 - Light Gray
        [0x4E, 0x4E, 0x4E, 0xFF], // 8 - Dark Gray
        [0x4E, 0x4E, 0xDC, 0xFF], // 9 - Light Blue
        [0x4E, 0xDC, 0x4E, 0xFF], // 10 - Light Green
        [0x4E, 0xF3, 0xF3, 0xFF], // 11 - Light Cyan
        [0xDC, 0x4E, 0x4E, 0xFF], // 12 - Light Red
        [0xF3, 0x4E, 0xF3, 0xFF], // 13 - Light Magenta
        [0xF3, 0xF3, 0x4E, 0xFF], // 14 - Yellow
        [0xFF, 0xFF, 0xFF, 0xFF], // 15 - White
    ],
];

// Little-endian
const CGA_RGBA_COLORS_U32: &[[u32; 16]; 2] = &[
    [
        u32::from_le_bytes(CGA_RGBA_COLORS[0][0]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][1]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][2]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][3]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][4]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][5]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][6]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][7]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][8]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][9]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][10]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][11]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][12]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][13]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][14]),
        u32::from_le_bytes(CGA_RGBA_COLORS[0][15]),
    ],
    // VileR's palette
    [
        u32::from_le_bytes(CGA_RGBA_COLORS[1][0]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][1]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][2]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][3]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][4]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][5]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][6]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][7]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][8]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][9]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][10]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][11]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][12]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][13]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][14]),
        u32::from_le_bytes(CGA_RGBA_COLORS[1][15]),
    ],
];

// Return a RGBA slice given a CGA color Enum
pub fn color_enum_to_rgba(color: &CGAColor) -> &'static [u8; 4] {
    
    match color {
        CGAColor::Black         => &[0x10u8, 0x10u8, 0x10u8, 0xFFu8], // Make slightly visible for debugging
        CGAColor::Blue          => &[0x00u8, 0x00u8, 0xAAu8, 0xFFu8],
        CGAColor::Green         => &[0x00u8, 0xAAu8, 0x00u8, 0xFFu8],
        CGAColor::Cyan          => &[0x00u8, 0xAAu8, 0xAAu8, 0xFFu8],
        CGAColor::Red           => &[0xAAu8, 0x00u8, 0x00u8, 0xFFu8],
        CGAColor::Magenta       => &[0xAAu8, 0x00u8, 0xAAu8, 0xFFu8],
        CGAColor::Brown         => &[0xAAu8, 0x55u8, 0x00u8, 0xFFu8],
        CGAColor::White         => &[0xAAu8, 0xAAu8, 0xAAu8, 0xFFu8],
        CGAColor::BlackBright   => &[0x55u8, 0x55u8, 0x55u8, 0xFFu8],
        CGAColor::BlueBright    => &[0x55u8, 0x55u8, 0xFFu8, 0xFFu8],
        CGAColor::GreenBright   => &[0x55u8, 0xFFu8, 0x55u8, 0xFFu8],
        CGAColor::CyanBright    => &[0x55u8, 0xFFu8, 0xFFu8, 0xFFu8],
        CGAColor::RedBright     => &[0xFFu8, 0x55u8, 0x55u8, 0xFFu8],
        CGAColor::MagentaBright => &[0xFFu8, 0x55u8, 0xFFu8, 0xFFu8],
        CGAColor::Yellow        => &[0xFFu8, 0xFFu8, 0x55u8, 0xFFu8],
        CGAColor::WhiteBright   => &[0xFFu8, 0xFFu8, 0xFFu8, 0xFFu8],
    }
}

pub fn get_ega_gfx_color16(bits: u8) -> &'static [u8; 4] {

    #[allow(clippy::unusual_byte_groupings)]
    match bits & 0b010_111 {
        0b000_000 => &[0x10, 0x10, 0x10, 0xFF], // Make slightly brighter for debugging
        0b000_001 => &[0x00, 0x00, 0xAA, 0xFF],
        0b000_010 => &[0x00, 0xAA, 0x00, 0xFF],
        0b000_011 => &[0x00, 0xAA, 0xAA, 0xFF],
        0b000_100 => &[0xAA, 0x00, 0x00, 0xFF],
        0b000_101 => &[0xAA, 0x00, 0xAA, 0xFF],
        0b000_110 => &[0xAA, 0x55, 0x00, 0xFF], // Brown instead of dark yellow
        0b000_111 => &[0xAA, 0xAA, 0xAA, 0xFF],
        0b010_000 => &[0x55, 0x55, 0x55, 0xFF],
        0b010_001 => &[0x55, 0x55, 0xFF, 0xFF],
        0b010_010 => &[0x55, 0xFF, 0x55, 0xFF],
        0b010_011 => &[0x55, 0xFF, 0xFF, 0xFF],
        0b010_100 => &[0xFF, 0x55, 0x55, 0xFF],
        0b010_101 => &[0xFF, 0x55, 0xFF, 0xFF],
        0b010_110 => &[0xFF, 0xFF, 0x55, 0xFF],
        0b010_111 => &[0xFF, 0xFF, 0xFF, 0xFF],
        _ => &[0x00, 0x00, 0x00, 0xFF], // Default black
    }
}

pub fn get_ega_gfx_color64(bits: u8) -> &'static [u8; 4] {

    #[allow(clippy::unusual_byte_groupings)]
    match bits {
        0b000_000 => &[0x10, 0x10, 0x10, 0xFF], // Make slightly brighter for debugging
        0b000_001 => &[0x00, 0x00, 0xAA, 0xFF],
        0b000_010 => &[0x00, 0xAA, 0x00, 0xFF],
        0b000_011 => &[0x00, 0xAA, 0xAA, 0xFF],
        0b000_100 => &[0xAA, 0x00, 0x00, 0xFF],
        0b000_101 => &[0xAA, 0x00, 0xAA, 0xFF],
        0b000_110 => &[0xAA, 0xAA, 0x00, 0xFF], 
        0b000_111 => &[0xAA, 0xAA, 0xAA, 0xFF],
        0b001_000 => &[0x00, 0x00, 0x55, 0xFF],
        0b001_001 => &[0x00, 0x00, 0xFF, 0xFF],
        0b001_010 => &[0x00, 0xAA, 0x55, 0xFF],
        0b001_011 => &[0x00, 0xAA, 0xFF, 0xFF],
        0b001_100 => &[0xAA, 0x00, 0x55, 0xFF],
        0b001_101 => &[0xAA, 0x00, 0xFF, 0xFF],
        0b001_110 => &[0xAA, 0xAA, 0x55, 0xFF],
        0b001_111 => &[0xAA, 0xAA, 0xFF, 0xFF],
        0b010_000 => &[0x00, 0x55, 0x00, 0xFF],
        0b010_001 => &[0x00, 0x55, 0xAA, 0xFF],
        0b010_010 => &[0x00, 0xFF, 0x00, 0xFF],
        0b010_011 => &[0x00, 0xFF, 0xAA, 0xFF],
        0b010_100 => &[0xAA, 0x55, 0x00, 0xFF],
        0b010_101 => &[0xAA, 0x55, 0xAA, 0xFF],
        0b010_110 => &[0xAA, 0xFF, 0x00, 0xFF],
        0b010_111 => &[0xAA, 0xFF, 0xAA, 0xFF],
        0b011_000 => &[0x00, 0x55, 0x55, 0xFF],
        0b011_001 => &[0x00, 0x55, 0xFF, 0xFF],
        0b011_010 => &[0x00, 0xFF, 0x55, 0xFF],
        0b011_011 => &[0x00, 0xFF, 0xFF, 0xFF],
        0b011_100 => &[0xAA, 0x55, 0x55, 0xFF],
        0b011_101 => &[0xAA, 0x55, 0xFF, 0xFF],
        0b011_110 => &[0xAA, 0xFF, 0x55, 0xFF],
        0b011_111 => &[0xAA, 0xFF, 0xFF, 0xFF],
        0b100_000 => &[0x55, 0x00, 0x00, 0xFF],
        0b100_001 => &[0x55, 0x00, 0xAA, 0xFF],
        0b100_010 => &[0x55, 0xAA, 0x00, 0xFF],
        0b100_011 => &[0x55, 0xAA, 0xAA, 0xFF],
        0b100_100 => &[0xFF, 0x00, 0x00, 0xFF],
        0b100_101 => &[0xFF, 0x00, 0xAA, 0xFF],
        0b100_110 => &[0xFF, 0xAA, 0x00, 0xFF],
        0b100_111 => &[0xFF, 0xAA, 0xAA, 0xFF],
        0b101_000 => &[0x55, 0x00, 0x55, 0xFF],
        0b101_001 => &[0x55, 0x00, 0xFF, 0xFF],
        0b101_010 => &[0x55, 0xAA, 0x55, 0xFF],
        0b101_011 => &[0x55, 0xAA, 0xFF, 0xFF],
        0b101_100 => &[0xFF, 0x00, 0x55, 0xFF],
        0b101_101 => &[0xFF, 0x00, 0xFF, 0xFF],
        0b101_110 => &[0xFF, 0xAA, 0x55, 0xFF],
        0b101_111 => &[0xFF, 0xAA, 0xFF, 0xFF],
        0b110_000 => &[0x55, 0x55, 0x00, 0xFF],
        0b110_001 => &[0x55, 0x55, 0xAA, 0xFF],
        0b110_010 => &[0x55, 0xFF, 0x00, 0xFF],
        0b110_011 => &[0x55, 0xFF, 0xAA, 0xFF],
        0b110_100 => &[0xFF, 0x55, 0x00, 0xFF],
        0b110_101 => &[0xFF, 0x55, 0xAA, 0xFF],
        0b110_110 => &[0xFF, 0xFF, 0x00, 0xFF],
        0b110_111 => &[0xFF, 0xFF, 0xAA, 0xFF],
        0b111_000 => &[0x55, 0x55, 0x55, 0xFF],
        0b111_001 => &[0x55, 0x55, 0xFF, 0xFF],
        0b111_010 => &[0x55, 0xFF, 0x55, 0xFF],
        0b111_011 => &[0x55, 0xFF, 0xFF, 0xFF],
        0b111_100 => &[0xFF, 0x55, 0x55, 0xFF],
        0b111_101 => &[0xFF, 0x55, 0xFF, 0xFF],
        0b111_110 => &[0xFF, 0xFF, 0x55, 0xFF],
        0b111_111 => &[0xFF, 0xFF, 0xFF, 0xFF],
        _ => &[0x10, 0x10, 0x10, 0xFF], // Default black
    }
}

pub fn get_cga_composite_color( bits: u8, palette: &CGAPalette ) -> &'static [u8; 4] {

    match (bits, palette) {

        (0b0000, CGAPalette::Monochrome(_)) => &[0x00, 0x00, 0x00, 0xFF], // Black
        (0b0001, CGAPalette::Monochrome(_)) => &[0x00, 0x68, 0x0C, 0xFF], // Forest Green
        (0b0010, CGAPalette::Monochrome(_)) => &[0x21, 0x2B, 0xBD, 0xFF], // Dark Blue
        (0b0011, CGAPalette::Monochrome(_)) => &[0x0D, 0x9E, 0xD5, 0xFF], // Cyan
        (0b0100, CGAPalette::Monochrome(_)) => &[0x85, 0x09, 0x6C, 0xFF], // Maroon
        (0b0101, CGAPalette::Monochrome(_)) => &[0x75, 0x73, 0x76, 0xFF], // Grey
        (0b0110, CGAPalette::Monochrome(_)) => &[0xAF, 0x36, 0xFF, 0xFF], // Magenta
        (0b0111, CGAPalette::Monochrome(_)) => &[0x9B, 0xA9, 0xFF, 0xFF], // Lilac
        (0b1000, CGAPalette::Monochrome(_)) => &[0x51, 0x47, 0x00, 0xFF], // Brown
        (0b1001, CGAPalette::Monochrome(_)) => &[0x42, 0xBD, 0x00, 0xFF], // Bright Green
        (0b1010, CGAPalette::Monochrome(_)) => &[0x51, 0x53, 0x51, 0xFF], // Darker Grey  0x70 0x74 0x70 actual values but this looks better in KQI
        (0b1011, CGAPalette::Monochrome(_)) => &[0x5D, 0xF4, 0x7A, 0xFF], // Lime Green
        (0b1100, CGAPalette::Monochrome(_)) => &[0xE5, 0x54, 0x1D, 0xFF], // Red-Orange
        (0b1101, CGAPalette::Monochrome(_)) => &[0xD7, 0xCB, 0x19, 0xFF], // Yellow
        (0b1110, CGAPalette::Monochrome(_)) => &[0xFF, 0x81, 0xF2, 0xFF], // Pink
        (0b1111, CGAPalette::Monochrome(_)) => &[0xFD, 0xFF, 0xFC, 0xFF], // White

        (0b0000, CGAPalette::MagentaCyanWhite(_)) => &[0x00, 0x00, 0x00, 0xFF], // Black
        (0b0001, CGAPalette::MagentaCyanWhite(_)) => &[0x00, 0x9A, 0xFF, 0xFF], // Blue #1
        (0b0010, CGAPalette::MagentaCyanWhite(_)) => &[0x00, 0x42, 0xFF, 0xFF], // Dark Blue
        (0b0011, CGAPalette::MagentaCyanWhite(_)) => &[0x00, 0x90, 0xFF, 0xFF], // Blue #2
        (0b0100, CGAPalette::MagentaCyanWhite(_)) => &[0xAA, 0x4C, 0x00, 0xFF], // Brown
        (0b0101, CGAPalette::MagentaCyanWhite(_)) => &[0x84, 0xFA, 0xD2, 0xFF], // Lime Green
        (0b0110, CGAPalette::MagentaCyanWhite(_)) => &[0xB9, 0xA2, 0xAD, 0xFF], // Gray
        (0b0111, CGAPalette::MagentaCyanWhite(_)) => &[0x96, 0xF0, 0xFF, 0xFF], // Pale Blue
        (0b1000, CGAPalette::MagentaCyanWhite(_)) => &[0xCD, 0x1F, 0x00, 0xFF], // Dark red
        (0b1001, CGAPalette::MagentaCyanWhite(_)) => &[0xA7, 0xCD, 0xFF, 0xFF], // Lilac #1
        (0b1010, CGAPalette::MagentaCyanWhite(_)) => &[0xDC, 0x75, 0xFF, 0xFF], // Magenta
        (0b1011, CGAPalette::MagentaCyanWhite(_)) => &[0xB9, 0xC3, 0xFF, 0xFF], // Lilac #2
        (0b1100, CGAPalette::MagentaCyanWhite(_)) => &[0xFF, 0x5C, 0x00, 0xFF], // Orange-Red
        (0b1101, CGAPalette::MagentaCyanWhite(_)) => &[0xED, 0xFF, 0xCC, 0xFF], // Pale yellow
        (0b1110, CGAPalette::MagentaCyanWhite(_)) => &[0xFF, 0xB2, 0xA6, 0xFF], // Peach
        (0b1111, CGAPalette::MagentaCyanWhite(_)) => &[0xFF, 0xFF, 0xFF, 0xFF], // White
        _ => &[0x00, 0x00, 0x00, 0xFF], // Default black
    }
}

pub fn get_cga_gfx_color(bits: u8, palette: &CGAPalette, intensity: bool) -> &'static [u8; 4] {
    match (bits, palette, intensity) {
        // Monochrome
        (0b00, CGAPalette::Monochrome(_), false) => &[0x00u8, 0x00u8, 0x00u8, 0x00u8], // Black
        (0b01, CGAPalette::Monochrome(fg), false) => color_enum_to_rgba(fg), // Foreground color
        // Palette 0 - Low Intensity
        (0b00, CGAPalette::RedGreenYellow(bg), false) => color_enum_to_rgba(bg), // Background color
        (0b01, CGAPalette::RedGreenYellow(_), false) => &[0x00u8, 0xAAu8, 0x00u8, 0xFFu8], // Green
        (0b10, CGAPalette::RedGreenYellow(_), false) => &[0xAAu8, 0x00u8, 0x00u8, 0xFFu8], // Red
        (0b11, CGAPalette::RedGreenYellow(_), false) => &[0xAAu8, 0x55u8, 0x00u8, 0xFFu8], // Brown
        // Palette 0 - High Intensity
        (0b00, CGAPalette::RedGreenYellow(bg), true) => color_enum_to_rgba(bg), // Background color
        (0b01, CGAPalette::RedGreenYellow(_), true) => &[0x55u8, 0xFFu8, 0x55u8, 0xFFu8], // GreenBright
        (0b10, CGAPalette::RedGreenYellow(_), true) => &[0xFFu8, 0x55u8, 0x55u8, 0xFFu8], // RedBright
        (0b11, CGAPalette::RedGreenYellow(_), true) => &[0xFFu8, 0xFFu8, 0x55u8, 0xFFu8], // Yellow
        // Palette 1 - Low Intensity
        (0b00, CGAPalette::MagentaCyanWhite(bg), false) => color_enum_to_rgba(bg), // Background color
        (0b01, CGAPalette::MagentaCyanWhite(_), false) => &[0x00u8, 0xAAu8, 0xAAu8, 0xFFu8], // Cyan
        (0b10, CGAPalette::MagentaCyanWhite(_), false) => &[0xAAu8, 0x00u8, 0xAAu8, 0xFFu8], // Magenta
        (0b11, CGAPalette::MagentaCyanWhite(_), false) => &[0xAAu8, 0xAAu8, 0xAAu8, 0xFFu8], // Gray
        // Palette 1 - High Intensity
        (0b00, CGAPalette::MagentaCyanWhite(bg), true) => color_enum_to_rgba(bg), // Background color
        (0b01, CGAPalette::MagentaCyanWhite(_), true) => &[0x55u8, 0xFFu8, 0xFFu8, 0xFFu8], // CyanBright
        (0b10, CGAPalette::MagentaCyanWhite(_), true) => &[0xFFu8, 0x55u8, 0xFFu8, 0xFFu8], // MagentaBright
        (0b11, CGAPalette::MagentaCyanWhite(_), true) => &[0xFFu8, 0xFFu8, 0xFFu8, 0xFFu8], // WhiteBright
        // Palette 2 - Low Intensity
        (0b00, CGAPalette::RedCyanWhite(bg), false) => color_enum_to_rgba(bg), // Background color
        (0b01, CGAPalette::RedCyanWhite(_), false) => &[0x00u8, 0xAAu8, 0xAAu8, 0xFFu8], // Cyan
        (0b10, CGAPalette::RedCyanWhite(_), false) => &[0xAAu8, 0x00u8, 0x00u8, 0xFFu8], // Red
        (0b11, CGAPalette::RedCyanWhite(_), false) => &[0xAAu8, 0xAAu8, 0xAAu8, 0xFFu8], // Gray
        // Palette 2 - High Intensity
        (0b00, CGAPalette::RedCyanWhite(bg), true) => color_enum_to_rgba(bg), // Background color
        (0b01, CGAPalette::RedCyanWhite(_), true) => &[0x55u8, 0xFFu8, 0xFFu8, 0xFFu8], // CyanBright
        (0b10, CGAPalette::RedCyanWhite(_), true) => &[0xFFu8, 0x55u8, 0x55u8, 0xFFu8], // RedBright
        (0b11, CGAPalette::RedCyanWhite(_), true) => &[0xFFu8, 0xFFu8, 0xFFu8, 0xFFu8], // WhiteBright
        _=> &[0x00u8, 0x00u8, 0x00u8, 0xFFu8] // Default Black
    }
}

pub struct VideoRenderer {
    mode: DisplayMode,
    cols: u32,
    rows: u32,

    composite_buf: Option<Vec<u8>>,
    composite_params: CompositeParams,
    sync_table_w: u32,
    sync_table: Vec<(f32, f32, f32)>
}

impl VideoRenderer {
    pub fn new(video_type: VideoType) -> Self {

        // Create a buffer to hold composite conversion of CGA graphics.
        // This buffer will need to be twice as large as the largest possible
        // CGA screen (CGA_MAX_CLOCK * 4) to account for half-hdots used in the 
        // composite conversion process.
        let composite_vec_opt = match video_type {
            VideoType::CGA => {
                Some(vec![0; cga::CGA_MAX_CLOCK * 4])
            }
            _ => {
                None
            }
        };

        Self {
            mode: DisplayMode::Mode3TextCo80,
            cols: 80,
            rows: 25,

            composite_buf: composite_vec_opt,
            composite_params: Default::default(),
            sync_table_w: 0,
            sync_table: Vec::new()
        }
    }

    /// Given the specified resolution and desired aspect ratio, return an aspect corrected resolution
    /// by adjusting the vertical resolution (Horizontal resolution will never be changed)
    pub fn get_aspect_corrected_res(res: (u32, u32), aspect: AspectRatio) -> (u32, u32) {

        let desired_ratio: f64 = aspect.h as f64 / aspect.v as f64;

        let adjusted_h = (res.0 as f64 / desired_ratio) as u32; // Result should be slightly larger than integer, ok to cast

        (res.0, adjusted_h)
    }

    pub fn draw(&self, frame: &mut [u8], video_card: Box<&dyn VideoCard>, bus: &BusInterface, composite: bool) {

        //let video_card = video.borrow();        
        let start_address = video_card.get_start_address() as usize;
        let mode_40_cols = video_card.is_40_columns();

        let (frame_w, frame_h) = video_card.get_display_size();

        match video_card.get_display_mode() {
            DisplayMode::Disabled => {
                // Blank screen here?
                return
            }
            DisplayMode::Mode0TextBw40 | DisplayMode::Mode1TextCo40 | DisplayMode::Mode2TextBw80 | DisplayMode::Mode3TextCo80 => {
                let video_type = video_card.get_video_type();
                let cursor = video_card.get_cursor_info();
                let char_height = video_card.get_character_height();
    
                // Start address is multiplied by two due to 2 bytes per character (char + attr)

                let video_mem = match video_type {
                    VideoType::MDA | VideoType::CGA | VideoType::EGA => {
                        bus.get_slice_at(cga::CGA_MEM_ADDRESS + start_address * 2, cga::CGA_MEM_SIZE)
                    }
                    VideoType::VGA => {
                        bus.get_slice_at(cga::CGA_MEM_ADDRESS + start_address * 2, cga::CGA_MEM_SIZE)
                        //video_mem = video_card.get_vram();
                    }
                };
                
                // Get font info from adapter
                let font_info = video_card.get_current_font();

                self.draw_text_mode(
                    video_type, 
                    cursor, 
                    frame, 
                    frame_w, 
                    frame_h, 
                    video_mem, 
                    char_height, 
                    mode_40_cols, 
                    &font_info );
            }
            DisplayMode::Mode4LowResGraphics | DisplayMode::Mode5LowResAltPalette => {
                let (palette, intensity) = video_card.get_cga_palette();

                let video_mem = bus.get_slice_at(cga::CGA_MEM_ADDRESS, cga::CGA_MEM_SIZE);
                if !composite {
                    //draw_cga_gfx_mode2x(frame, frame_w, frame_h, video_mem, palette, intensity);
                    draw_cga_gfx_mode(frame, frame_w, frame_h, video_mem, palette, intensity);
                }
                else {
                    //draw_gfx_mode2x_composite(frame, frame_w, frame_h, video_mem, palette, intensity);
                }
            }
            DisplayMode::Mode6HiResGraphics => {
                let (palette, _intensity) = video_card.get_cga_palette();

                let video_mem = bus.get_slice_at(cga::CGA_MEM_ADDRESS, cga::CGA_MEM_SIZE);
                if !composite {
                    //draw_cga_gfx_mode_highres2x(frame, frame_w, frame_h, video_mem, palette);
                    draw_cga_gfx_mode_highres(frame, frame_w, frame_h, video_mem, palette);
                }
                else {
                    //draw_gfx_mode2x_composite(frame, frame_w, frame_h, video_mem, palette, intensity);
                }
                
            }
            DisplayMode::Mode7LowResComposite => {
                let (palette, _intensity) = video_card.get_cga_palette();

                let video_mem = bus.get_slice_at(cga::CGA_MEM_ADDRESS, cga::CGA_MEM_SIZE);
                if !composite {
                    //draw_cga_gfx_mode_highres2x(frame, frame_w, frame_h, video_mem, palette);
                    draw_cga_gfx_mode_highres(frame, frame_w, frame_h, video_mem, palette);
                }
                else {
                    //draw_gfx_mode2x_composite(frame, frame_w, frame_h, video_mem, palette, intensity);
                }                
            }
            DisplayMode::ModeDEGALowResGraphics => {
                draw_ega_lowres_gfx_mode(video_card, frame, frame_w, frame_h);
            }
            DisplayMode::Mode10EGAHiResGraphics => {
                draw_ega_hires_gfx_mode(video_card, frame, frame_w, frame_h);
            }
            DisplayMode::Mode12VGAHiResGraphics => {
                draw_vga_hires_gfx_mode(video_card, frame, frame_w, frame_h)
            }            
            DisplayMode::Mode13VGALowRes256 => {
                draw_vga_mode13h(video_card, frame, frame_w, frame_h);
            }

            _ => {
                // blank screen here?
            }
        }
    }

    pub fn screenshot(
        &self,
        frame: &mut [u8],
        frame_w: u32, 
        frame_h: u32,
        path: &Path) 
    {

        // Find first unique filename in screenshot dir
        let filename = file_util::find_unique_filename(path, "screenshot", ".png");

        match image::save_buffer(
            filename.clone(),
            frame,
            frame_w,
            frame_h, 
            image::ColorType::Rgba8) 
        {
            Ok(_) => println!("Saved screenshot: {}", filename.display()),
            Err(e) => {
                println!("Error writing screenshot: {}: {}", filename.display(), e)
            }
        }
    }

    pub fn draw_text_mode(
        &self, 
        video_type: VideoType,
        cursor: CursorInfo, 
        frame: &mut [u8], 
        frame_w: u32, 
        frame_h: u32, 
        mem: &[u8], 
        char_height: u8, 
        lowres: bool,
        font: &FontInfo ) 
    {

        let mem_span = match lowres {
            true => 40,
            false => 80
        };

        // Avoid drawing weird sizes during BIOS setup
        if frame_h < 200 {
            return
        }

        if char_height < 2 {
            return
        }

        let char_height = char_height as u32;

        let max_y = frame_h / char_height - 1;

        for (i, char) in mem.chunks_exact(2).enumerate() {
            let x = (i % mem_span as usize) as u32;
            let y = (i / mem_span as usize) as u32;
            
            //println!("x: {} y: {}", x, y);
            //pixel.copy_from_slice(&rgba);
            if y > max_y {
                break;
            }

            let (fg_color, bg_color) = get_colors_from_attr_byte(char[1]);

            match (video_type, lowres) {
                (VideoType::CGA, true) => {
                    draw_glyph4x(char[0], fg_color, bg_color, frame, frame_w, frame_h, char_height, x * 8, y * char_height, font)
                }
                (VideoType::CGA, false) => {
                    //draw_glyph2x(char[0], fg_color, bg_color, frame, frame_w, frame_h, char_height, x * 8, y * char_height, font)
                    draw_glyph1x1(char[0], fg_color, bg_color, frame, frame_w, frame_h, char_height, x * 8, y * char_height, font)
                }
                (VideoType::EGA, true) => {
                    draw_glyph2x1(
                        char[0], 
                        fg_color, 
                        bg_color, 
                        frame, 
                        frame_w, 
                        frame_h, 
                        char_height, 
                        x * 8 * 2, 
                        y * char_height, 
                        font)
                }
                (VideoType::EGA, false) => {
                    draw_glyph1x1(
                        char[0], 
                        fg_color, 
                        bg_color, 
                        frame, 
                        frame_w, 
                        frame_h, 
                        char_height, 
                        x * 8, 
                        y * char_height, 
                        font)                    
                }
                (VideoType::VGA, false) => {
                    draw_glyph1x1(
                        char[0], 
                        fg_color, 
                        bg_color, 
                        frame, 
                        frame_w, 
                        frame_h, 
                        char_height, 
                        x * 8, 
                        y * char_height, 
                        font)                    
                }
                _=> {}
            }

        }

        match (video_type, lowres) {
            (VideoType::CGA, true) => draw_cursor4x(cursor, frame, frame_w, frame_h, mem, font ),
            (VideoType::CGA, false) => {
                //draw_cursor2x(cursor, frame, frame_w, frame_h, mem, font ),
                draw_cursor(cursor, frame, frame_w, frame_h, mem, font )
            }
            (VideoType::EGA, true) | (VideoType::EGA, false) => {
                draw_cursor(cursor, frame, frame_w, frame_h, mem, font )
            }
            _=> {}
        }
    }

    pub fn draw_horizontal_xor_line(
        &mut self,
        frame: &mut [u8],
        w: u32,
        span: u32,
        h: u32,
        y: u32
    ) {

        if y > (h-1) {
            return;
        }

        let frame_row0_offset = ((y * 2) * (span * 4)) as usize;
        let frame_row1_offset = (((y * 2) * (span * 4)) + (span * 4)) as usize;

        for x in 0..w {

            let fo0 = frame_row0_offset + (x * 4) as usize;
            let fo1 = frame_row1_offset + (x * 4) as usize;

            let r = frame[fo0];
            let g = frame[fo0 + 1];
            let b = frame[fo0 + 2];

            frame[fo1] = r ^ XOR_COLOR;
            frame[fo1 + 1] = g ^ XOR_COLOR;
            frame[fo1 + 2] = b ^ XOR_COLOR;
        }
    }

    pub fn draw_vertical_xor_line(
        &mut self,
        frame: &mut [u8],
        w: u32,
        span: u32,
        h: u32,
        x: u32
    ) {

        if x > (w-1) {
            return;
        }

        let frame_x0_offset = (x * 4) as usize;

        for y in 0..h {
            let fo0 = frame_x0_offset + ((y * 2) * (span * 4)) as usize;
            let fo1 = frame_x0_offset + (((y * 2) * (span * 4)) + (span * 4)) as usize;

            let r = frame[fo0];
            let g = frame[fo0 + 1];
            let b = frame[fo0 + 2];

            frame[fo0] = r ^ XOR_COLOR;
            frame[fo0 + 1] = g ^ XOR_COLOR;
            frame[fo0 + 2] = b ^ XOR_COLOR;

            frame[fo1] = r ^ XOR_COLOR;
            frame[fo1 + 1] = g ^ XOR_COLOR;
            frame[fo1 + 2] = b ^ XOR_COLOR;
        }

    }    

    /// Set the alpha component of each pixel in a the specified buffer.
    pub fn set_alpha(
        frame: &mut [u8],
        w: u32,
        h: u32,
        a: u8
    ) {
        //log::warn!("set_alpha: h: {}", h);

        for o in (0..((w*h*4) as usize)).step_by(4) {
            frame[o + 3] = a;
        }
    }

    /// Draw the CGA card in Direct Mode. 
    /// Cards in Direct Mode generate their own framebuffers, we simply display the current back buffer
    /// Optionally composite processing is performed.
    pub fn draw_cga_direct(
        &mut self,
        frame: &mut [u8],
        w: u32,
        h: u32,
        dbuf: &[u8],
        extents: &DisplayExtents,
        composite_enabled: bool,
        composite_params: &CompositeParams,
        beam_pos: Option<(u32, u32)>
    ) {

        if composite_enabled {
            self.draw_cga_direct_composite(frame, w, h, dbuf, extents, composite_params);
            return
        }

        // Attempt to center the image by reducing right overscan 
        //let overscan_total = extents.aperture_w.saturating_sub(extents.visible_w);
        //let overscan_half = overscan_total / 2;

        let mut horiz_adjust = extents.aperture_x;
        if extents.aperture_x + extents.aperture_w >= extents.field_w {
            horiz_adjust = 0;
        }
        /*
        if overscan_half < extents.overscan_l {
            // We want to shift image to the right 
            horiz_adjust = extents.overscan_l - overscan_half;
        }
        */

        // Assume display buffer visible data starts at offset 0

        let max_y = std::cmp::min(h / 2, extents.aperture_h);
        let max_x = std::cmp::min(w, extents.aperture_w);

        //log::debug!("w: {w} h: {h} max_x: {max_x}, max_y: {max_y}");

        for y in 0..max_y {

            let dbuf_row_offset = y as usize * extents.row_stride;
            let frame_row0_offset = ((y * 2) * (w * 4)) as usize;
            let frame_row1_offset = (((y * 2) * (w * 4)) + (w * 4)) as usize;

            for x in 0..max_x {
                let fo0 = frame_row0_offset + (x * 4) as usize;
                let fo1 = frame_row1_offset + (x * 4) as usize;

                let dbo = dbuf_row_offset + (x + horiz_adjust) as usize;

                frame[fo0]       = CGA_RGBA_COLORS[0][(dbuf[dbo] & 0x0F) as usize][0];
                frame[fo0 + 1]   = CGA_RGBA_COLORS[0][(dbuf[dbo] & 0x0F) as usize][1];
                frame[fo0 + 2]   = CGA_RGBA_COLORS[0][(dbuf[dbo] & 0x0F) as usize][2];
                frame[fo0 + 3]   = 0xFFu8;

                frame[fo1]       = CGA_RGBA_COLORS[0][(dbuf[dbo] & 0x0F) as usize][0];
                frame[fo1 + 1]   = CGA_RGBA_COLORS[0][(dbuf[dbo] & 0x0F) as usize][1];
                frame[fo1 + 2]   = CGA_RGBA_COLORS[0][(dbuf[dbo] & 0x0F) as usize][2];
                frame[fo1 + 3]   = 0xFFu8;                
            }
        }

        // Draw crosshairs for debugging crt beam pos
        if let Some(beam) = beam_pos {
            self.draw_horizontal_xor_line(frame, w, max_x, max_y, beam.1);
            self.draw_vertical_xor_line(frame, w, max_x, max_y, beam.0);
        }
    }

    /// Draw the CGA card in Direct Mode. 
    /// Cards in Direct Mode generate their own framebuffers, we simply display the current back buffer
    /// Optionally composite processing is performed.
    pub fn draw_cga_direct_u32(
        &mut self,
        frame: &mut [u8],
        w: u32,
        h: u32,
        dbuf: &[u8],
        extents: &DisplayExtents,
        composite_enabled: bool,
        composite_params: &CompositeParams,
        beam_pos: Option<(u32, u32)>
    ) {

        if composite_enabled {
            self.draw_cga_direct_composite_u32(frame, w, h, dbuf, extents, composite_params);
            return
        }

        // Attempt to center the image by reducing right overscan 
        //let overscan_total = extents.aperture_w.saturating_sub(extents.visible_w);
        //let overscan_half = overscan_total / 2;

        let mut horiz_adjust = extents.aperture_x;
        if extents.aperture_x + extents.aperture_w >= extents.field_w {
            horiz_adjust = 0;
        }
        /*
        if overscan_half < extents.overscan_l {
            // We want to shift image to the right 
            horiz_adjust = extents.overscan_l - overscan_half;
        }
        */

        // Assume display buffer visible data starts at offset 0

        let max_y = std::cmp::min(h / 2, extents.aperture_h);
        let max_x = std::cmp::min(w, extents.aperture_w);

        //log::debug!("w: {w} h: {h} max_x: {max_x}, max_y: {max_y}");

        let frame_u32: &mut [u32] = bytemuck::cast_slice_mut(frame);

        for y in 0..max_y {

            let dbuf_row_offset = y as usize * (extents.row_stride / 4);
            let frame_row0_offset = ((y * 2) * w) as usize;
            let frame_row1_offset = (((y * 2) * w) + (w)) as usize;

            for x in 0..max_x {
                let fo0 = frame_row0_offset + x as usize;
                let fo1 = frame_row1_offset + x as usize;

                let dbo = dbuf_row_offset + (x + horiz_adjust) as usize;

                frame_u32[fo0] = CGA_RGBA_COLORS_U32[0][(dbuf[dbo] & 0x0F) as usize];
                frame_u32[fo1] = CGA_RGBA_COLORS_U32[0][(dbuf[dbo] & 0x0F) as usize];
            }
        }

        // Draw crosshairs for debugging crt beam pos
        if let Some(beam) = beam_pos {
            self.draw_horizontal_xor_line(frame, w, max_x, max_y, beam.1);
            self.draw_vertical_xor_line(frame, w, max_x, max_y, beam.0);
        }
    }    

    pub fn draw_cga_direct_composite(
        &mut self,
        frame: &mut [u8],
        w: u32,
        h: u32,        
        dbuf: &[u8],
        extents: &DisplayExtents,
        composite_params: &CompositeParams
    ) {

        if let Some(composite_buf) = &mut self.composite_buf {
            let max_w = std::cmp::min(w, extents.aperture_w);
            let max_h = std::cmp::min(h / 2, extents.aperture_h);
            
            //log::debug!("composite: w: {w} h: {h} max_w: {max_w}, max_h: {max_h}");
            //log::debug!("composite: aperture.x: {}", extents.aperture_x);

            process_cga_composite_int(
                dbuf, 
                extents.aperture_w, 
                extents.aperture_h, 
                extents.aperture_x,
                extents.aperture_y,
                extents.row_stride as u32, 
                composite_buf);

            // Regen sync table if width changed
            if self.sync_table_w != (max_w * 2) {
                self.sync_table.resize(((max_w * 2) + CCYCLE as u32) as usize, (0.0, 0.0, 0.0));
                regen_sync_table(&mut self.sync_table,(max_w * 2) as usize);
                // Update to new width
                self.sync_table_w = max_w * 2;
            }

            artifact_colors_fast(
                composite_buf, 
                max_w * 2, 
                max_h, 
                &self.sync_table, 
                frame, 
                max_w, 
                max_h, 
                composite_params.hue, 
                composite_params.sat,
                composite_params.luma
            );
        }
    }

    pub fn draw_cga_direct_composite_u32(
        &mut self,
        frame: &mut [u8],
        w: u32,
        h: u32,        
        dbuf: &[u8],
        extents: &DisplayExtents,
        composite_params: &CompositeParams
    ) {

        if let Some(composite_buf) = &mut self.composite_buf {
            let max_w = std::cmp::min(w, extents.aperture_w);
            let max_h = std::cmp::min(h / 2, extents.aperture_h);
            
            //log::debug!("composite: w: {w} h: {h} max_w: {max_w}, max_h: {max_h}");

            process_cga_composite_int(
                dbuf, 
                extents.aperture_w, 
                extents.aperture_h, 
                extents.overscan_l,
                extents.overscan_t,
                extents.row_stride as u32, 
                composite_buf);

            // Regen sync table if width changed
            if self.sync_table_w != (max_w * 2) {
                self.sync_table.resize(((max_w * 2) + CCYCLE as u32) as usize, (0.0, 0.0, 0.0));
                regen_sync_table(&mut self.sync_table,(max_w * 2) as usize);
                // Update to new width
                self.sync_table_w = max_w * 2;
            }

            artifact_colors_fast_u32(
                composite_buf, 
                max_w * 2, 
                max_h, 
                &self.sync_table, 
                frame, 
                max_w, 
                max_h, 
                composite_params.hue, 
                composite_params.sat,
                composite_params.luma
            );
        }
    }

}

pub fn draw_cga_gfx_mode(frame: &mut [u8], frame_w: u32, _frame_h: u32, mem: &[u8], pal: CGAPalette, intensity: bool) {
    // First half of graphics memory contains all EVEN rows (0, 2, 4, 6, 8)
    let mut field_src_offset = 0;
    let mut field_dst_offset = 0;
    for _field in 0..2 {
        for draw_y in 0..(CGA_GFX_H / 2) {

            // CGA gfx mode = 2 bits (4 pixels per byte). Double line count to skip every other line
            let src_y_idx = draw_y * (CGA_GFX_W / 4) + field_src_offset; 
            let dst_span = frame_w * 4;
            let dst1_y_idx = draw_y * dst_span * 2 + field_dst_offset;  // RBGA = 4 bytes

            // Draw 4 pixels at a time
            for draw_x in 0..(CGA_GFX_W / 4) {

                let dst1_x_idx = (draw_x * 4) * 4;
                //let dst2_x_idx = dst1_x_idx + 4;

                let cga_byte: u8 = mem[(src_y_idx + draw_x) as usize];

                // Four pixels in a byte
                for pix_n in 0..4 {
                    // Mask the pixel bits, right-to-left
                    let shift_ct = 8 - (pix_n * 2) - 2;
                    let pix_bits = cga_byte >> shift_ct & 0x03;
                    // Get the RGBA for this pixel
                    let color = get_cga_gfx_color(pix_bits, &pal, intensity);

                    let draw_offset = (dst1_y_idx + dst1_x_idx + (pix_n * 4)) as usize;
                    if draw_offset + 3 < frame.len() {
                        frame[draw_offset]     = color[0];
                        frame[draw_offset + 1] = color[1];
                        frame[draw_offset + 2] = color[2];
                        frame[draw_offset + 3] = color[3];
                    }                       
                }
            }
        }
        // Switch fields
        field_src_offset += CGA_FIELD_OFFSET;
        field_dst_offset += frame_w * 4;
    }
}

pub fn draw_cga_gfx_mode2x(frame: &mut [u8], frame_w: u32, _frame_h: u32, mem: &[u8], pal: CGAPalette, intensity: bool) {
    // First half of graphics memory contains all EVEN rows (0, 2, 4, 6, 8)
    
    let mut field_src_offset = 0;
    let mut field_dst_offset = 0;
    for _field in 0..2 {
        for draw_y in 0..(CGA_GFX_H / 2) {

            // CGA gfx mode = 2 bits (4 pixels per byte). Double line count to skip every other line
            let src_y_idx = draw_y * (CGA_GFX_W / 4) + field_src_offset; 
            let dst_span = (frame_w) * 4;
            let dst1_y_idx = draw_y * (dst_span * 4) + field_dst_offset;  // RBGA = 4 bytes x 2x pixels
            let dst2_y_idx = draw_y * (dst_span * 4) + dst_span + field_dst_offset;  // One scanline down

            // Draw 4 pixels at a time
            for draw_x in 0..(CGA_GFX_W / 4) {

                let dst1_x_idx = (draw_x * 4) * 4 * 2;
                let dst2_x_idx = dst1_x_idx + 4;

                let cga_byte: u8 = mem[(src_y_idx + draw_x) as usize];

                // Four pixels in a byte
                for pix_n in 0..4 {
                    // Mask the pixel bits, right-to-left
                    let shift_ct = 8 - (pix_n * 2) - 2;
                    let pix_bits = cga_byte >> shift_ct & 0x03;
                    // Get the RGBA for this pixel
                    let color = get_cga_gfx_color(pix_bits, &pal, intensity);
                    // Draw first row of pixel 2x
                    frame[(dst1_y_idx + dst1_x_idx + (pix_n * 8)) as usize]     = color[0];
                    frame[(dst1_y_idx + dst1_x_idx + (pix_n * 8)) as usize + 1] = color[1];
                    frame[(dst1_y_idx + dst1_x_idx + (pix_n * 8)) as usize + 2] = color[2];
                    frame[(dst1_y_idx + dst1_x_idx + (pix_n * 8)) as usize + 3] = color[3];

                    frame[(dst1_y_idx + dst2_x_idx + (pix_n * 8)) as usize]     = color[0];
                    frame[(dst1_y_idx + dst2_x_idx + (pix_n * 8)) as usize + 1] = color[1];
                    frame[(dst1_y_idx + dst2_x_idx + (pix_n * 8)) as usize + 2] = color[2];
                    frame[(dst1_y_idx + dst2_x_idx + (pix_n * 8)) as usize + 3] = color[3];

                    // Draw 2nd row of pixel 2x
                    frame[(dst2_y_idx + dst1_x_idx + (pix_n * 8)) as usize]     = color[0];
                    frame[(dst2_y_idx + dst1_x_idx + (pix_n * 8)) as usize + 1] = color[1];
                    frame[(dst2_y_idx + dst1_x_idx + (pix_n * 8)) as usize + 2] = color[2];
                    frame[(dst2_y_idx + dst1_x_idx + (pix_n * 8)) as usize + 3] = color[3];      

                    frame[(dst2_y_idx + dst2_x_idx + (pix_n * 8)) as usize]     = color[0];
                    frame[(dst2_y_idx + dst2_x_idx + (pix_n * 8)) as usize + 1] = color[1];
                    frame[(dst2_y_idx + dst2_x_idx + (pix_n * 8)) as usize + 2] = color[2];
                    frame[(dst2_y_idx + dst2_x_idx + (pix_n * 8)) as usize + 3] = color[3];                                    
                }
            }
        }
        field_src_offset += CGA_FIELD_OFFSET;
        field_dst_offset += (frame_w) * 4 * 2;
    }
}

pub fn draw_cga_gfx_mode_highres(frame: &mut [u8], frame_w: u32, _frame_h: u32, mem: &[u8], pal: CGAPalette) {
    // First half of graphics memory contains all EVEN rows (0, 2, 4, 6, 8)
    
    let mut field_src_offset = 0;
    let mut field_dst_offset = 0;
    for _field in 0..2 {
        for draw_y in 0..(CGA_HIRES_GFX_H / 2) {

            // CGA hi-res gfx mode = 1 bpp (8 pixels per byte).
            let src_y_idx = draw_y * (CGA_HIRES_GFX_W / 8) + field_src_offset; 
            let dst_span = frame_w * 4;
            let dst1_y_idx = draw_y * dst_span * 2 + field_dst_offset;  // RBGA = 4 bytes
            //let dst2_y_idx = draw_y * (dst_span * 4) + dst_span + field_dst_offset;  // One scanline down

            // Draw 8 pixels at a time
            for draw_x in 0..(CGA_HIRES_GFX_W / 8) {

                let dst1_x_idx = (draw_x * 8) * 4;

                let cga_byte: u8 = mem[(src_y_idx + draw_x) as usize];

                // Eight pixels in a byte
                for pix_n in 0..8 {
                    // Mask the pixel bits, right-to-left
                    let shift_ct = 8 - pix_n - 1;
                    let pix_bit = cga_byte >> shift_ct & 0x01;
                    // Get the RGBA for this pixel
                    let color = get_cga_gfx_color(pix_bit, &pal, false);
                    // Draw first row of pixel
                    let draw_offset = (dst1_y_idx + dst1_x_idx + (pix_n * 4)) as usize;
                    if draw_offset + 3 < frame.len() {
                        frame[draw_offset + 0] = color[0];
                        frame[draw_offset + 1] = color[1];
                        frame[draw_offset + 2] = color[2];
                        frame[draw_offset + 3] = color[3];
                    }     
                }
            }
        }
        field_src_offset += CGA_FIELD_OFFSET;
        field_dst_offset += frame_w * 4;
    }
}

pub fn draw_cga_gfx_mode_highres2x(frame: &mut [u8], frame_w: u32, _frame_h: u32, mem: &[u8], pal: CGAPalette) {
    // First half of graphics memory contains all EVEN rows (0, 2, 4, 6, 8)
    
    let mut field_src_offset = 0;
    let mut field_dst_offset = 0;
    for _field in 0..2 {
        for draw_y in 0..(CGA_HIRES_GFX_H / 2) {

            // CGA hi-res gfx mode = 1 bpp (8 pixels per byte).

            let src_y_idx = draw_y * (CGA_HIRES_GFX_W / 8) + field_src_offset; 

            let dst_span = frame_w * 4;
            let dst1_y_idx = draw_y * (dst_span * 4) + field_dst_offset;  // RBGA = 4 bytes x 2x pixels
            let dst2_y_idx = draw_y * (dst_span * 4) + dst_span + field_dst_offset;  // One scanline down

            // Draw 8 pixels at a time
            for draw_x in 0..(CGA_HIRES_GFX_W / 8) {

                let dst1_x_idx = (draw_x * 8) * 4;

                let cga_byte: u8 = mem[(src_y_idx + draw_x) as usize];

                // Eight pixels in a byte
                for pix_n in 0..8 {
                    // Mask the pixel bits, right-to-left
                    let shift_ct = 8 - pix_n - 1;
                    let pix_bit = cga_byte >> shift_ct & 0x01;
                    // Get the RGBA for this pixel
                    let color = get_cga_gfx_color(pix_bit, &pal, false);
                    // Draw first row of pixel
                    frame[(dst1_y_idx + dst1_x_idx + (pix_n * 4)) as usize]     = color[0];
                    frame[(dst1_y_idx + dst1_x_idx + (pix_n * 4)) as usize + 1] = color[1];
                    frame[(dst1_y_idx + dst1_x_idx + (pix_n * 4)) as usize + 2] = color[2];
                    frame[(dst1_y_idx + dst1_x_idx + (pix_n * 4)) as usize + 3] = color[3];

                    // Draw 2nd row of pixel
                    frame[(dst2_y_idx + dst1_x_idx + (pix_n * 4)) as usize]     = color[0];
                    frame[(dst2_y_idx + dst1_x_idx + (pix_n * 4)) as usize + 1] = color[1];
                    frame[(dst2_y_idx + dst1_x_idx + (pix_n * 4)) as usize + 2] = color[2];
                    frame[(dst2_y_idx + dst1_x_idx + (pix_n * 4)) as usize + 3] = color[3];      
                }
            }
        }
        field_src_offset += CGA_FIELD_OFFSET;
        field_dst_offset += (frame_w) * 4 * 2;
    }
}


pub fn draw_gfx_mode2x_composite(frame: &mut [u8], frame_w: u32, _frame_h: u32, mem: &[u8], pal: CGAPalette, _intensity: bool) {
    // First half of graphics memory contains all EVEN rows (0, 2, 4, 6, 8)
    
    let mut field_src_offset = 0;
    let mut field_dst_offset = 0;
    for _field in 0..2 {
        for draw_y in 0..(CGA_GFX_H / 2) {

            // CGA gfx mode = 2 bits (4 pixels per byte). Double line count to skip every other line
            let src_y_idx = draw_y * (CGA_GFX_W / 4) + field_src_offset; 
            let dst_span = (frame_w) * 4;
            let dst1_y_idx = draw_y * (dst_span * 4) + field_dst_offset;  // RBGA = 4 bytes x 2x pixels
            let dst2_y_idx = draw_y * (dst_span * 4) + dst_span + field_dst_offset;  // One scanline down

            // Draw 4 pixels at a time
            for draw_x in 0..(CGA_GFX_W / 4) {

                let dst1_x_idx = (draw_x * 4) * 4 * 2;
                let dst2_x_idx = dst1_x_idx + 4;
                let dst3_x_idx = dst1_x_idx + 8;
                let dst4_x_idx = dst1_x_idx + 12;

                let cga_byte: u8 = mem[(src_y_idx + draw_x) as usize];

                // Two composite 'pixels' in a byte
                for pix_n in 0..2 {
                    // Mask the pixel bits, right-to-left
                    let shift_ct = 8 - (pix_n * 4) - 4;
                    let pix_bits = cga_byte >> shift_ct & 0x0F;
                    // Get the RGBA for this pixel
                    let color = get_cga_composite_color(pix_bits, &pal);
                    // Draw first row of pixel 4x
                    frame[(dst1_y_idx + dst1_x_idx + (pix_n * 16)) as usize]     = color[0];
                    frame[(dst1_y_idx + dst1_x_idx + (pix_n * 16)) as usize + 1] = color[1];
                    frame[(dst1_y_idx + dst1_x_idx + (pix_n * 16)) as usize + 2] = color[2];
                    frame[(dst1_y_idx + dst1_x_idx + (pix_n * 16)) as usize + 3] = color[3];

                    frame[(dst1_y_idx + dst2_x_idx + (pix_n * 16)) as usize]     = color[0];
                    frame[(dst1_y_idx + dst2_x_idx + (pix_n * 16)) as usize + 1] = color[1];
                    frame[(dst1_y_idx + dst2_x_idx + (pix_n * 16)) as usize + 2] = color[2];
                    frame[(dst1_y_idx + dst2_x_idx + (pix_n * 16)) as usize + 3] = color[3];

                    frame[(dst1_y_idx + dst3_x_idx + (pix_n * 16)) as usize]     = color[0];
                    frame[(dst1_y_idx + dst3_x_idx + (pix_n * 16)) as usize + 1] = color[1];
                    frame[(dst1_y_idx + dst3_x_idx + (pix_n * 16)) as usize + 2] = color[2];
                    frame[(dst1_y_idx + dst3_x_idx + (pix_n * 16)) as usize + 3] = color[3];
                    
                    frame[(dst1_y_idx + dst4_x_idx + (pix_n * 16)) as usize]     = color[0];
                    frame[(dst1_y_idx + dst4_x_idx + (pix_n * 16)) as usize + 1] = color[1];
                    frame[(dst1_y_idx + dst4_x_idx + (pix_n * 16)) as usize + 2] = color[2];
                    frame[(dst1_y_idx + dst4_x_idx + (pix_n * 16)) as usize + 3] = color[3];                    

                    // Draw 2nd row of pixel 4x
                    frame[(dst2_y_idx + dst1_x_idx + (pix_n * 16)) as usize]     = color[0];
                    frame[(dst2_y_idx + dst1_x_idx + (pix_n * 16)) as usize + 1] = color[1];
                    frame[(dst2_y_idx + dst1_x_idx + (pix_n * 16)) as usize + 2] = color[2];
                    frame[(dst2_y_idx + dst1_x_idx + (pix_n * 16)) as usize + 3] = color[3];      

                    frame[(dst2_y_idx + dst2_x_idx + (pix_n * 16)) as usize]     = color[0];
                    frame[(dst2_y_idx + dst2_x_idx + (pix_n * 16)) as usize + 1] = color[1];
                    frame[(dst2_y_idx + dst2_x_idx + (pix_n * 16)) as usize + 2] = color[2];
                    frame[(dst2_y_idx + dst2_x_idx + (pix_n * 16)) as usize + 3] = color[3];      

                    frame[(dst2_y_idx + dst3_x_idx + (pix_n * 16)) as usize]     = color[0];
                    frame[(dst2_y_idx + dst3_x_idx + (pix_n * 16)) as usize + 1] = color[1];
                    frame[(dst2_y_idx + dst3_x_idx + (pix_n * 16)) as usize + 2] = color[2];
                    frame[(dst2_y_idx + dst3_x_idx + (pix_n * 16)) as usize + 3] = color[3];    

                    frame[(dst2_y_idx + dst4_x_idx + (pix_n * 16)) as usize]     = color[0];
                    frame[(dst2_y_idx + dst4_x_idx + (pix_n * 16)) as usize + 1] = color[1];
                    frame[(dst2_y_idx + dst4_x_idx + (pix_n * 16)) as usize + 2] = color[2];
                    frame[(dst2_y_idx + dst4_x_idx + (pix_n * 16)) as usize + 3] = color[3];    
                }
            }
        }
        field_src_offset += CGA_FIELD_OFFSET;
        field_dst_offset += (frame_w) * 4 * 2;
    }
}

pub fn get_colors_from_attr_byte(byte: u8) -> (CGAColor, CGAColor) {

    let fg_nibble = byte & 0x0F;
    let bg_nibble = (byte >> 4 ) & 0x0F;

    let bg_color = get_colors_from_attr_nibble(bg_nibble);
    let fg_color = get_colors_from_attr_nibble(fg_nibble);

    (fg_color, bg_color)
}

pub fn get_colors_from_attr_nibble(byte: u8) -> CGAColor {

    match byte {
        0b0000 => CGAColor::Black,
        0b0001 => CGAColor::Blue,
        0b0010 => CGAColor::Green,
        0b0100 => CGAColor::Red,
        0b0011 => CGAColor::Cyan,
        0b0101 => CGAColor::Magenta,
        0b0110 => CGAColor::Brown,
        0b0111 => CGAColor::White,
        0b1000 => CGAColor::BlackBright,
        0b1001 => CGAColor::BlueBright,
        0b1010 => CGAColor::GreenBright,
        0b1100 => CGAColor::RedBright,
        0b1011 => CGAColor::CyanBright,
        0b1101 => CGAColor::MagentaBright,
        0b1110 => CGAColor::Yellow,
        0b1111 => CGAColor::WhiteBright,
        _=> CGAColor::Black
    }
}

// Draw a CGA font glyph in 40 column mode at an arbitrary location
pub fn draw_glyph4x( 
    glyph: u8,
    fg_color: CGAColor,
    bg_color: CGAColor,
    frame: &mut [u8], 
    frame_w: u32, 
    frame_h: u32, 
    char_height: u32,
    pos_x: u32, 
    pos_y: u32,
    font: &FontInfo )
{

    // Do not draw glyph off screen
    if (pos_x + (font.w * 2) > frame_w) || (pos_y * 2 + (font.h * 2 ) > frame_h) {
        return
    }

    // Find the source position of the glyph
    //let glyph_offset_src_x = glyph as u32 % FONT_SPAN;
    //let glyph_offset_src_y = (glyph as u32 / FONT_SPAN) * (FONT_H * FONT_SPAN); 
    let glyph_offset_src_x = glyph as u32;
    let glyph_offset_src_y = 0;

    let max_char_height = std::cmp::min(font.h, char_height);
    for draw_glyph_y in 0..max_char_height {

        let dst_row_offset = frame_w * 4 * ((pos_y * 2) + (draw_glyph_y*2));
        let dst_row_offset2 = dst_row_offset + (frame_w * 4);
        
        let glyph_offset = glyph_offset_src_y + (draw_glyph_y * 256) + glyph_offset_src_x;

        let glyph_byte: u8 = font.font_data[glyph_offset as usize];

        for draw_glyph_x in 0..font.w {
        
            let test_bit: u8 = 0x80u8 >> draw_glyph_x;

            let color = if test_bit & glyph_byte > 0 {
                color_enum_to_rgba(&fg_color)
            }
            else {
                color_enum_to_rgba(&bg_color)
            };

            let dst_offset = dst_row_offset + ((pos_x * 2) + (draw_glyph_x*2)) * 4;
            frame[dst_offset as usize] = color[0];
            frame[dst_offset as usize + 1] = color[1];
            frame[dst_offset as usize + 2] = color[2];
            frame[dst_offset as usize + 3] = color[3];

            frame[(dst_offset + 4) as usize] = color[0];
            frame[(dst_offset + 4) as usize + 1] = color[1];
            frame[(dst_offset + 4) as usize + 2] = color[2];
            frame[(dst_offset + 4) as usize + 3] = color[3];


            let dst_offset2 = dst_row_offset2 + ((pos_x * 2) + (draw_glyph_x*2)) * 4;
            frame[dst_offset2 as usize] = color[0];
            frame[dst_offset2 as usize + 1] = color[1];
            frame[dst_offset2 as usize + 2] = color[2];
            frame[dst_offset2 as usize + 3] = color[3];   

            frame[(dst_offset2 + 4 ) as usize] = color[0];
            frame[(dst_offset2 + 4) as usize + 1] = color[1];
            frame[(dst_offset2 + 4) as usize + 2] = color[2];
            frame[(dst_offset2 + 4) as usize + 3] = color[3];    
        }
    }     
}

// Draw a CGA font glyph in 80 column mode at an arbitrary location
pub fn draw_glyph2x( 
    glyph: u8,
    fg_color: CGAColor,
    bg_color: CGAColor,
    frame: &mut [u8], 
    frame_w: u32, 
    frame_h: u32, 
    char_height: u32,
    pos_x: u32, 
    pos_y: u32,
    font: &FontInfo ) 
{

    // Do not draw glyph off screen
    if pos_x + font.w > frame_w {
        return
    }
    if pos_y * 2 + (font.h * 2 ) > frame_h {
        return
    }

    // Find the source position of the glyph

    //let glyph_offset_src_x = glyph as u32 % FONT_SPAN;
    //let glyph_offset_src_y = (glyph as u32 / FONT_SPAN) * (FONT_H * FONT_SPAN); 
    let glyph_offset_src_x = glyph as u32;
    let glyph_offset_src_y = 0;

    let max_char_height = std::cmp::min(font.h, char_height);
    for draw_glyph_y in 0..max_char_height {

        let dst_row_offset = frame_w * 4 * ((pos_y * 2) + (draw_glyph_y*2));
        let dst_row_offset2 = dst_row_offset + (frame_w * 4);
        
        let glyph_offset = glyph_offset_src_y + (draw_glyph_y * 256) + glyph_offset_src_x;

        let glyph_byte: u8 = font.font_data[glyph_offset as usize];

        for draw_glyph_x in 0..font.w {
        
            let test_bit: u8 = 0x80u8 >> draw_glyph_x;

            let color = if test_bit & glyph_byte > 0 {
                color_enum_to_rgba(&fg_color)
            }
            else {
                color_enum_to_rgba(&bg_color)
            };

            let dst_offset = dst_row_offset + (pos_x + draw_glyph_x) * 4;
            frame[dst_offset as usize] = color[0];
            frame[dst_offset as usize + 1] = color[1];
            frame[dst_offset as usize + 2] = color[2];
            frame[dst_offset as usize + 3] = color[3];

            let dst_offset2 = dst_row_offset2 + (pos_x + draw_glyph_x) * 4;
            frame[dst_offset2 as usize] = color[0];
            frame[dst_offset2 as usize + 1] = color[1];
            frame[dst_offset2 as usize + 2] = color[2];
            frame[dst_offset2 as usize + 3] = color[3];            
        }
    }     
}

pub fn draw_cursor4x(cursor: CursorInfo, frame: &mut [u8], frame_w: u32, frame_h: u32, mem: &[u8], font: &FontInfo ) {
        
    // First off, is cursor even visible?
    if !cursor.visible {
        return
    }
    
    // Do not draw cursor off screen
    let pos_x = cursor.pos_x * font.w;
    let pos_y = cursor.pos_y * font.h;
    if (pos_x + (font.w * 2) > frame_w) || (pos_y * 2 + (font.h * 2 ) > frame_h) {
        return
    }

    // Cursor start register can be greater than end register, in this case no cursor is shown
    if cursor.line_start > cursor.line_end {
        return
    }

    let line_start = cursor.line_start as u32;
    let mut line_end = cursor.line_end as u32;

    // Clip cursor if at bottom of screen and cursor.line_end > FONT_H
    if pos_y * 2 + line_end * 2 >= frame_h {
        line_end -= frame_h - (pos_y * 2 + line_end * 2) + 1;
    }        

    // Is character attr in mem range?
    let attr_addr = (cursor.addr * 2 + 1) as usize;
    if attr_addr > mem.len() {
        return
    }
    let cursor_attr: u8 = mem[attr_addr];
    let (fg_color, _bg_color) = get_colors_from_attr_byte(cursor_attr);
    let color = color_enum_to_rgba(&fg_color);

    for draw_glyph_y in line_start..line_end {

        let dst_row_offset = frame_w * 4 * ((pos_y * 2) + (draw_glyph_y*2));
        let dst_row_offset2 = dst_row_offset + (frame_w * 4);
        
        for draw_glyph_x in 0..font.w {
        
            let dst_offset = dst_row_offset + ((pos_x * 2) + (draw_glyph_x*2)) * 4;
            frame[dst_offset as usize] = color[0];
            frame[dst_offset as usize + 1] = color[1];
            frame[dst_offset as usize + 2] = color[2];
            frame[dst_offset as usize + 3] = color[3];

            frame[(dst_offset + 4) as usize] = color[0];
            frame[(dst_offset + 4) as usize + 1] = color[1];
            frame[(dst_offset + 4) as usize + 2] = color[2];
            frame[(dst_offset + 4) as usize + 3] = color[3];

            let dst_offset2 = dst_row_offset2 + ((pos_x * 2) + (draw_glyph_x*2)) * 4;
            frame[dst_offset2 as usize] = color[0];
            frame[dst_offset2 as usize + 1] = color[1];
            frame[dst_offset2 as usize + 2] = color[2];
            frame[dst_offset2 as usize + 3] = color[3];   

            frame[(dst_offset2 + 4 ) as usize] = color[0];
            frame[(dst_offset2 + 4) as usize + 1] = color[1];
            frame[(dst_offset2 + 4) as usize + 2] = color[2];
            frame[(dst_offset2 + 4) as usize + 3] = color[3];    
        }
    }    
}

/// Draw the cursor as a character cell into the specified framebuffer with 2x height
pub fn draw_cursor2x(cursor: CursorInfo, frame: &mut [u8], frame_w: u32, frame_h: u32, mem: &[u8] , font: &FontInfo ) {
    
    // First off, is cursor even visible?
    if !cursor.visible {
        return
    }
    
    // Do not draw cursor off screen
    let pos_x = cursor.pos_x * font.w;
    let pos_y = cursor.pos_y * font.h;

    let max_pos_x = pos_x + font.w; 
    let max_pos_y = pos_y * 2 + (font.h * 2);  
    if max_pos_x > frame_w || max_pos_y > frame_h {
        return
    }

    // Cursor start register can be greater than end register, in this case no cursor is shown
    if cursor.line_start > cursor.line_end {
        return
    }

    let line_start = cursor.line_start as u32;
    let mut line_end = cursor.line_end as u32;

    // Clip cursor if at bottom of screen and cursor.line_end > FONT_H
    if pos_y * 2 + line_end * 2 >= frame_h {
        line_end -= frame_h - (pos_y * 2 + line_end * 2) + 1;
    }

    // Is character attr in mem range?
    let attr_addr = (cursor.addr * 2 + 1) as usize;
    if attr_addr > mem.len() {
        return
    }
    let cursor_attr: u8 = mem[attr_addr];
    let (fg_color, _bg_color) = get_colors_from_attr_byte(cursor_attr);
    let color = color_enum_to_rgba(&fg_color);

    for draw_glyph_y in line_start..=line_end {

        let dst_row_offset = frame_w * 4 * ((pos_y * 2) + (draw_glyph_y*2));
        let dst_row_offset2 = dst_row_offset + (frame_w * 4);
                                    
        for draw_glyph_x in 0..font.w {
        
            let dst_offset = dst_row_offset + (pos_x + draw_glyph_x) * 4;
            frame[dst_offset as usize] = color[0];
            frame[dst_offset as usize + 1] = color[1];
            frame[dst_offset as usize + 2] = color[2];
            frame[dst_offset as usize + 3] = color[3];

            let dst_offset2 = dst_row_offset2 + (pos_x + draw_glyph_x) * 4;
            frame[dst_offset2 as usize] = color[0];
            frame[dst_offset2 as usize + 1] = color[1];
            frame[dst_offset2 as usize + 2] = color[2];
            frame[dst_offset2 as usize + 3] = color[3];   

        }
    }                 
}

/// Draw the cursor as a character cell into the specified framebuffer at native height
pub fn draw_cursor(cursor: CursorInfo, frame: &mut [u8], frame_w: u32, frame_h: u32, mem: &[u8] , font: &FontInfo ) {
    
    // First off, is cursor even visible?
    if !cursor.visible {
        return
    }
    
    // Do not draw cursor off screen
    let pos_x = cursor.pos_x * font.w;
    let pos_y = cursor.pos_y * font.h;

    let max_pos_x = pos_x + font.w; 
    let max_pos_y = pos_y + font.h;  
    if max_pos_x > frame_w || max_pos_y > frame_h {
        return
    }

    // Cursor start register can be greater than end register, in this case no cursor is shown
    if cursor.line_start > cursor.line_end {
        return
    }

    let line_start = cursor.line_start as u32;
    let mut line_end = cursor.line_end as u32;

    // Clip cursor if at bottom of screen and cursor.line_end > FONT_H
    if pos_y + line_end >= frame_h {
        line_end -= frame_h - (pos_y + line_end) + 1;
    }

    // Is character attr in mem range?
    let attr_addr = (cursor.addr * 2 + 1) as usize;
    if attr_addr > mem.len() {
        return
    }
    let cursor_attr: u8 = mem[attr_addr];
    let (fg_color, _bg_color) = get_colors_from_attr_byte(cursor_attr);
    let color = color_enum_to_rgba(&fg_color);

    for draw_glyph_y in line_start..=line_end {

        let dst_row_offset = frame_w * 4 * (pos_y + draw_glyph_y);
        for draw_glyph_x in 0..font.w {
        
            let dst_offset = dst_row_offset + (pos_x + draw_glyph_x) * 4;
            frame[dst_offset as usize] = color[0];
            frame[dst_offset as usize + 1] = color[1];
            frame[dst_offset as usize + 2] = color[2];
            frame[dst_offset as usize + 3] = color[3];
        }
    }                 
}

// Draw a font glyph at an arbitrary location at 2x horizontal resolution
pub fn draw_glyph2x1( 
    glyph: u8,
    fg_color: CGAColor,
    bg_color: CGAColor,
    frame: &mut [u8], 
    frame_w: u32, 
    frame_h: u32, 
    char_height: u32,
    pos_x: u32, 
    pos_y: u32,
    font: &FontInfo )
{

    // Do not draw a glyph off screen
    if pos_x + (font.w * 2) > frame_w {
        return
    }
    if pos_y + font.h > frame_h {
        return
    }

    // Find the source position of the glyph
    //let glyph_offset_src_x = glyph as u32 % FONT_SPAN;
    //let glyph_offset_src_y = (glyph as u32 / FONT_SPAN) * (FONT_H * FONT_SPAN); 
    let glyph_offset_src_x = glyph as u32;
    let glyph_offset_src_y = 0;

    let max_char_height = std::cmp::min(font.h, char_height);
    for draw_glyph_y in 0..max_char_height {

        let dst_row_offset = frame_w * 4 * (pos_y + draw_glyph_y);
        //let glyph_offset = glyph_offset_src_y + (draw_glyph_y * FONT_SPAN) + glyph_offset_src_x;
        let glyph_offset = glyph_offset_src_y + (draw_glyph_y * 256) + glyph_offset_src_x;

        let glyph_byte: u8 = font.font_data[glyph_offset as usize];

        for draw_glyph_x in 0..font.w {
        
            let test_bit: u8 = 0x80u8 >> draw_glyph_x;

            let color = if test_bit & glyph_byte > 0 {
                color_enum_to_rgba(&fg_color)
            }
            else {
                color_enum_to_rgba(&bg_color)
            };

            let dst_offset = dst_row_offset + (pos_x + draw_glyph_x * 2) * 4;
            frame[dst_offset as usize + 0] = color[0];
            frame[dst_offset as usize + 1] = color[1];
            frame[dst_offset as usize + 2] = color[2];
            frame[dst_offset as usize + 3] = color[3];

            frame[dst_offset as usize + 4] = color[0];
            frame[dst_offset as usize + 5] = color[1];
            frame[dst_offset as usize + 6] = color[2];
            frame[dst_offset as usize + 7] = color[3];            
        }
    }
}

// Draw a font glyph at an arbitrary location at normal resolution
pub fn draw_glyph1x1( 
    glyph: u8,
    fg_color: CGAColor,
    bg_color: CGAColor,
    frame: &mut [u8], 
    frame_w: u32, 
    frame_h: u32, 
    char_height: u32,
    pos_x: u32, 
    pos_y: u32,
    font: &FontInfo )
{

    // Do not draw glyph off screen
    if pos_x + font.w > frame_w {
        return
    }
    if pos_y + font.h > frame_h {
        return
    }

    // Find the source position of the glyph
    //let glyph_offset_src_x = glyph as u32 % FONT_SPAN;
    //let glyph_offset_src_y = (glyph as u32 / FONT_SPAN) * (FONT_H * FONT_SPAN); 
    let glyph_offset_src_x = glyph as u32;
    let glyph_offset_src_y = 0;

    let max_char_height = std::cmp::min(font.h, char_height);
    for draw_glyph_y in 0..max_char_height {

        let dst_row_offset = frame_w * 4 * (pos_y + draw_glyph_y);
        //let glyph_offset = glyph_offset_src_y + (draw_glyph_y * FONT_SPAN) + glyph_offset_src_x;
        let glyph_offset = glyph_offset_src_y + (draw_glyph_y * 256) + glyph_offset_src_x;

        let glyph_byte: u8 = font.font_data[glyph_offset as usize];

        for draw_glyph_x in 0..font.w {
        
            let test_bit: u8 = 0x80u8 >> draw_glyph_x;

            let color = if test_bit & glyph_byte > 0 {
                color_enum_to_rgba(&fg_color)
            }
            else {
                color_enum_to_rgba(&bg_color)
            };

            let dst_offset = dst_row_offset + (pos_x + draw_glyph_x) * 4;
            frame[dst_offset as usize] = color[0];
            frame[dst_offset as usize + 1] = color[1];
            frame[dst_offset as usize + 2] = color[2];
            frame[dst_offset as usize + 3] = color[3];
        }
    }
}





pub fn draw_ega_lowres_gfx_mode(ega: Box<&dyn VideoCard>, frame: &mut [u8], frame_w: u32, _frame_h: u32 ) {

    for draw_y in 0..EGA_LORES_GFX_H {

        let dst_span = frame_w * 4;
        let dst1_y_idx = draw_y * dst_span;

        for draw_x in 0..EGA_LORES_GFX_W {

            let dst1_x_idx = draw_x * 4;

            let ega_bits = ega.get_pixel_raw(draw_x, draw_y);
            //if ega_bits != 0 {
            //  log::trace!("ega bits: {:06b}", ega_bits);
            //}
            let color = get_ega_gfx_color16(ega_bits);

            let draw_offset = (dst1_y_idx + dst1_x_idx) as usize;
            if draw_offset + 3 < frame.len() {
                frame[draw_offset + 0] = color[0];
                frame[draw_offset + 1] = color[1];
                frame[draw_offset + 2] = color[2];
                frame[draw_offset + 3] = color[3];
            }
        }
    }
}

pub fn draw_ega_hires_gfx_mode(ega: Box<&dyn VideoCard>, frame: &mut [u8], frame_w: u32, _frame_h: u32 ) {

    for draw_y in 0..EGA_HIRES_GFX_H {

        let dst_span = frame_w * 4;
        let dst1_y_idx = draw_y * dst_span;

        for draw_x in 0..EGA_HIRES_GFX_W {

            let dst1_x_idx = draw_x * 4;

            let ega_bits = ega.get_pixel_raw(draw_x, draw_y);

            // High resolution mode offers the entire 64 color palette
            let color = get_ega_gfx_color64(ega_bits);

            let draw_offset = (dst1_y_idx + dst1_x_idx) as usize;
            if draw_offset + 3 < frame.len() {
                frame[draw_offset + 0] = color[0];
                frame[draw_offset + 1] = color[1];
                frame[draw_offset + 2] = color[2];
                frame[draw_offset + 3] = color[3];
            }
        }
    }
}

pub fn draw_vga_hires_gfx_mode(vga: Box<&dyn VideoCard>, frame: &mut [u8], frame_w: u32, _frame_h: u32 ) {

    for draw_y in 0..VGA_HIRES_GFX_H {

        let dst_span = frame_w * 4;
        let dst1_y_idx = draw_y * dst_span;

        for draw_x in 0..VGA_HIRES_GFX_W {

            let dst1_x_idx = draw_x * 4;

            let rgba = vga.get_pixel(draw_x, draw_y);
            
            let draw_offset = (dst1_y_idx + dst1_x_idx) as usize;
            if draw_offset + 3 < frame.len() {
                frame[draw_offset + 0] = rgba[0];
                frame[draw_offset + 1] = rgba[1];
                frame[draw_offset + 2] = rgba[2];
                frame[draw_offset + 3] = rgba[3];
            }
        }
    }
}


/// Draw Video memory in VGA Mode 13h (320x200@256 colors)
/// 
/// This mode is actually 640x400, double-scanned horizontally and vertically
pub fn draw_vga_mode13h(vga: Box<&dyn VideoCard>, frame: &mut [u8], frame_w: u32, _frame_h: u32 ) {

    for draw_y in 0..VGA_LORES_GFX_H {

        let dst_span = frame_w * 4;
        let dst1_y_idx = draw_y * 2 * dst_span;
        let dst2_y_idx = dst1_y_idx + dst_span;

        for draw_x in 0..VGA_LORES_GFX_W {

            let dst1_x_idx = draw_x * 4 * 2;

            let color = vga.get_pixel(draw_x, draw_y);

            let draw_offset = (dst1_y_idx + dst1_x_idx) as usize;
            let draw_offset2 = (dst2_y_idx + dst1_x_idx) as usize;
            if draw_offset2 + 3 < frame.len() {

                frame[draw_offset + 0] = color[0];
                frame[draw_offset + 1] = color[1];
                frame[draw_offset + 2] = color[2];
                frame[draw_offset + 3] = 0xFF;
                frame[draw_offset + 4] = color[0];
                frame[draw_offset + 5] = color[1];
                frame[draw_offset + 6] = color[2];
                frame[draw_offset + 7] = 0xFF;

                frame[draw_offset2 + 0] = color[0];
                frame[draw_offset2 + 1] = color[1];
                frame[draw_offset2 + 2] = color[2];
                frame[draw_offset2 + 3] = 0xFF;  
                frame[draw_offset2 + 4] = color[0];
                frame[draw_offset2 + 5] = color[1];
                frame[draw_offset2 + 6] = color[2];
                frame[draw_offset2 + 7] = 0xFF;                                 
            }
        }
    }
}