use crate::dom::{DomTreeView, NodeId};
use crate::style::{ComputedStyle, Edges};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x
            && p.x <= self.x + self.width
            && p.y >= self.y
            && p.y <= self.y + self.height
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    pub fn inset(&self, edges: Edges) -> Self {
        let x = self.x + edges.left;
        let y = self.y + edges.top;
        let width = (self.width - edges.left - edges.right).max(0.0);
        let height = (self.height - edges.top - edges.bottom).max(0.0);
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Resolved absolute and relative layout geometry for a DOM node computed via Taffy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayoutBox {
    /// Absolute bounding rect in surface coordinates (border-box).
    pub border_box: Rect,
    /// Absolute content box inside padding and borders.
    pub content_box: Rect,
    pub padding: Edges,
    pub border: Edges,
    pub margin: Edges,
    pub z_index: i32,
}

/// RGBA8 pixel buffer rendered for a shell surface or window decoration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameBuffer {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8 bytes (`width * height * 4`).
    pub pixels: Vec<u8>,
    pub damaged_rects: Vec<(i32, i32, u32, u32)>,
}

impl FrameBuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0u8; (width * height * 4) as usize],
            damaged_rects: vec![(0, 0, width, height)],
        }
    }

    /// Returns the number of non-transparent pixels in the buffer (useful for testing).
    pub fn non_transparent_pixel_count(&self) -> usize {
        self.pixels
            .chunks_exact(4)
            .filter(|px| px[3] > 0)
            .count()
    }
}

/// Trait implemented by `crates/layout-engine`.
pub trait LayoutSolver: Send + Sync {
    fn compute_layout(
        &mut self,
        dom: &dyn DomTreeView,
        styles: &HashMap<NodeId, ComputedStyle>,
        viewport: Size,
    ) -> HashMap<NodeId, LayoutBox>;

    /// Hit-tests a point against computed layout boxes in reverse paint order (`z_index` aware).
    fn hit_test(
        &self,
        dom: &dyn DomTreeView,
        layouts: &HashMap<NodeId, LayoutBox>,
        point: Point,
    ) -> Option<NodeId>;
}

/// Trait implemented by `crates/renderer`.
pub trait SurfaceRenderer: Send + Sync {
    fn render_surface(
        &mut self,
        dom: &dyn DomTreeView,
        styles: &HashMap<NodeId, ComputedStyle>,
        layouts: &HashMap<NodeId, LayoutBox>,
        viewport: Size,
    ) -> FrameBuffer;
}
