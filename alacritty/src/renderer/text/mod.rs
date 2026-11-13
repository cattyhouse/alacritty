use std::mem;

use bitflags::bitflags;
use crossfont::{GlyphKey, RasterizedGlyph};

use alacritty_terminal::term::cell::Flags;

use crate::display::SizeInfo;
use crate::display::content::RenderableCell;
use crate::gl;
use crate::gl::types::*;

mod atlas;
mod builtin_font;
mod gles2;
mod glsl3;
pub mod glyph_cache;

use atlas::Atlas;
pub use gles2::Gles2Renderer;
pub use glsl3::Glsl3Renderer;
pub use glyph_cache::GlyphCache;
use glyph_cache::{Glyph, LoadGlyph};

// NOTE: These flags must be in sync with their usage in the text.*.glsl shaders.
bitflags! {
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct RenderingGlyphFlags: u8 {
        const COLORED   = 0b0000_0001;
        const WIDE_CHAR = 0b0000_0010;
    }
}

/// Rendering passes, for both GLES2 and GLSL3 renderer.
#[repr(u8)]
enum RenderingPass {
    /// Rendering pass used to render background color in text shaders.
    Background = 0,

    /// The first pass to render text with both GLES2 and GLSL3 renderers.
    SubpixelPass1 = 1,

    /// The second pass to render text with GLES2 renderer.
    SubpixelPass2 = 2,

    /// The third pass to render text with GLES2 renderer.
    SubpixelPass3 = 3,
}

pub trait TextRenderer<'a> {
    type Shader: TextShader;
    type RenderBatch: TextRenderBatch;
    type RenderApi: TextRenderApi<Self::RenderBatch>;

    /// Get loader API for the renderer.
    fn loader_api(&mut self) -> LoaderApi<'_>;

    /// Draw cells.
    fn draw_cells<'b: 'a, I: Iterator<Item = RenderableCell>>(
        &'b mut self,
        size_info: &'b SizeInfo,
        glyph_cache: &'a mut GlyphCache,
        cells: I,
    ) {
        self.with_api(size_info, |mut api| {
            // Resolve every glyph up front, then draw the cells grouped by the atlas texture
            // they ended up in.
            //
            // Without this the batch is flushed at every texture boundary, because screen
            // traversal order (row by row) does not match the order glyphs were allocated into
            // atlas pages. With a lot of CJK text that turns a frame into dozens of tiny draws.
            //
            // Grouping is safe because cells never overlap on screen, so the draw order within
            // a frame is not observable.
            let mut cells: Vec<ResolvedCell> =
                cells.map(|cell| api.resolve_cell(cell, glyph_cache)).collect();

            // The buffer is taken out of the API so the borrow ends before drawing, then put
            // back to reuse its capacity on the next frame.
            let mut scratch = mem::take(api.scratch());
            group_by_texture(&mut cells, &mut scratch);

            for resolved in scratch.frame_cells() {
                api.add_render_item(&resolved.cell, &resolved.primary, size_info);
                for glyph in &resolved.zerowidth {
                    api.add_render_item(&resolved.cell, glyph, size_info);
                }
            }

            *api.scratch() = scratch;
        })
    }

    fn with_api<'b: 'a, F, T>(&'b mut self, size_info: &'b SizeInfo, func: F) -> T
    where
        F: FnOnce(Self::RenderApi) -> T;

    fn program(&self) -> &Self::Shader;

    /// Resize the text rendering.
    fn resize(&self, size: &SizeInfo) {
        unsafe {
            let program = self.program();
            gl::UseProgram(program.id());
            update_projection(program.projection_uniform(), size);
            gl::UseProgram(0);
        }
    }

    /// Invoke renderer with the loader.
    fn with_loader<F: FnOnce(LoaderApi<'_>) -> T, T>(&mut self, func: F) -> T {
        unsafe {
            gl::ActiveTexture(gl::TEXTURE0);
        }

        func(self.loader_api())
    }
}

/// A cell with every glyph it needs already resolved, ready to be grouped by texture.
#[derive(Debug)]
pub struct ResolvedCell {
    cell: RenderableCell,
    primary: Glyph,
    /// Combining marks drawn on top of `primary`.
    ///
    /// Empty for almost every cell, and `Vec::new` does not allocate, so resolving a frame
    /// costs no per-cell heap traffic on the common path.
    zerowidth: Vec<Glyph>,
}

impl ResolvedCell {
    /// The atlas texture this cell's glyphs all live in, or `None` when they are spread out.
    pub fn single_texture(&self) -> Option<GLuint> {
        if self.zerowidth.is_empty()
            || self.zerowidth.iter().all(|g| g.tex_id == self.primary.tex_id)
        {
            Some(self.primary.tex_id)
        } else {
            None
        }
    }
}

/// One atlas texture and the cells which can be drawn in a single batch.
#[derive(Debug)]
struct GlyphBucket {
    tex_id: GLuint,
    /// Indices into [`TextureGroupedCells::cells`], in screen order.
    entries: Vec<usize>,
}

/// Cells of one frame, grouped by the atlas texture they can be drawn with.
///
/// This lives in the renderer rather than being rebuilt every frame, so the allocations are
/// reused as long as the number of visible cells does not grow.
#[derive(Debug, Default)]
pub struct TextureGroupedCells {
    cells: Vec<ResolvedCell>,
    buckets: Vec<GlyphBucket>,
    /// Indices of cells whose glyphs span several textures, in screen order.
    ungrouped: Vec<usize>,
    /// Emission order: every bucket in turn, then the ungrouped cells.
    order: Vec<usize>,
}

impl TextureGroupedCells {
    /// Cells in the order they must be drawn.
    pub fn frame_cells(&self) -> impl Iterator<Item = &ResolvedCell> {
        self.order.iter().map(|index| &self.cells[*index])
    }

    /// Drop the previous frame's cells, keeping the allocated capacity.
    pub fn clear(&mut self) {
        self.cells.clear();
        self.buckets.clear();
        self.ungrouped.clear();
        self.order.clear();
    }
}

/// Group the given cells by atlas texture, appending the draw order to `out`.
///
/// Only cells whose glyphs all live in the same texture are grouped. Anything mixed (for
/// example a base glyph plus zero-width combining marks from another atlas page) keeps its
/// original position at the end, so it can never be drawn before its base glyph.
pub fn group_by_texture(cells: &mut Vec<ResolvedCell>, out: &mut TextureGroupedCells) {
    out.clear();
    out.cells.append(cells);

    for index in 0..out.cells.len() {
        match out.cells[index].single_texture() {
            Some(tex_id) => match out.buckets.iter_mut().find(|bucket| bucket.tex_id == tex_id) {
                Some(bucket) => bucket.entries.push(index),
                None => out.buckets.push(GlyphBucket { tex_id, entries: vec![index] }),
            },
            None => out.ungrouped.push(index),
        }
    }

    out.order.reserve(out.cells.len());
    for bucket in &out.buckets {
        out.order.extend_from_slice(&bucket.entries);
    }
    out.order.extend_from_slice(&out.ungrouped);
}

pub trait TextRenderBatch {
    /// Check if `Batch` is empty.
    fn is_empty(&self) -> bool;

    /// Check whether the `Batch` is full.
    fn full(&self) -> bool;

    /// Get texture `Batch` is using.
    fn tex(&self) -> GLuint;

    /// Add item to the batch.
    fn add_item(&mut self, cell: &RenderableCell, glyph: &Glyph, size_info: &SizeInfo);
}

pub trait TextRenderApi<T: TextRenderBatch>: LoadGlyph {
    /// Get `Batch` the api is using.
    fn batch(&mut self) -> &mut T;

    /// Get the reusable per-frame grouping buffer.
    fn scratch(&mut self) -> &mut TextureGroupedCells;

    /// Render the underlying data.
    fn render_batch(&mut self);

    /// Add item to the rendering queue.
    #[inline]
    fn add_render_item(&mut self, cell: &RenderableCell, glyph: &Glyph, size_info: &SizeInfo) {
        // Flush batch if tex changing.
        if !self.batch().is_empty() && self.batch().tex() != glyph.tex_id {
            self.render_batch();
        }

        self.batch().add_item(cell, glyph, size_info);

        // Render batch and clear if it's full.
        if self.batch().full() {
            self.render_batch();
        }
    }

    /// Resolve all glyphs a cell needs without drawing anything.
    ///
    /// Returns the cell together with its glyphs: the primary one first, followed by any visible
    /// zero-width characters.
    fn resolve_cell(
        &mut self,
        mut cell: RenderableCell,
        glyph_cache: &mut GlyphCache,
    ) -> ResolvedCell {
        let font_key = match cell.flags & Flags::BOLD_ITALIC {
            Flags::BOLD_ITALIC => glyph_cache.bold_italic_key,
            Flags::ITALIC => glyph_cache.italic_key,
            Flags::BOLD => glyph_cache.bold_key,
            _ => glyph_cache.font_key,
        };

        let hidden = cell.flags.contains(Flags::HIDDEN);
        if cell.character == '\t' || hidden {
            cell.character = ' ';
        }

        let mut glyph_key =
            GlyphKey { font_key, size: glyph_cache.font_size, character: cell.character };

        let primary = glyph_cache.get(glyph_key, self, true);

        // `Vec::new` does not allocate, and the overwhelming majority of cells have no
        // combining marks, so the common path stays free of per-cell heap traffic.
        let mut zerowidth = Vec::new();

        if let Some(marks) =
            cell.extra.as_mut().and_then(|extra| extra.zerowidth.take().filter(|_| !hidden))
        {
            for character in marks {
                glyph_key.character = character;
                zerowidth.push(glyph_cache.get(glyph_key, self, false));
            }
        }

        ResolvedCell { cell, primary, zerowidth }
    }
}

pub trait TextShader {
    fn id(&self) -> GLuint;

    /// Id of the projection uniform.
    fn projection_uniform(&self) -> GLint;
}

#[derive(Debug)]
pub struct LoaderApi<'a> {
    active_tex: &'a mut GLuint,
    atlas: &'a mut Vec<Atlas>,
    current_atlas: &'a mut usize,
}

impl LoadGlyph for LoaderApi<'_> {
    fn load_glyph(&mut self, rasterized: &RasterizedGlyph) -> Glyph {
        Atlas::load_glyph(self.active_tex, self.atlas, self.current_atlas, rasterized)
    }

    fn clear(&mut self) {
        Atlas::clear_atlas(self.atlas, self.current_atlas)
    }
}

fn update_projection(u_projection: GLint, size: &SizeInfo) {
    let width = size.width();
    let height = size.height();
    let padding_x = size.padding_x();
    let padding_y = size.padding_y();

    // Bounds check.
    if (width as u32) < (2 * padding_x as u32) || (height as u32) < (2 * padding_y as u32) {
        return;
    }

    // Compute scale and offset factors, from pixel to ndc space. Y is inverted.
    //   [0, width - 2 * padding_x] to [-1, 1]
    //   [height - 2 * padding_y, 0] to [-1, 1]
    let scale_x = 2. / (width - 2. * padding_x);
    let scale_y = -2. / (height - 2. * padding_y);
    let offset_x = -1.;
    let offset_y = 1.;

    unsafe {
        gl::Uniform4f(u_projection, offset_x, offset_y, scale_x, scale_y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alacritty_terminal::index::{Column, Point};

    /// Build a resolved cell whose glyphs live in the given textures.
    ///
    /// The first texture is the primary glyph, the rest are combining marks.
    fn cell(id: u32, textures: &[GLuint]) -> ResolvedCell {
        let glyph = |tex_id: GLuint| Glyph {
            tex_id,
            multicolor: false,
            top: 0,
            left: 0,
            width: 0,
            height: 0,
            uv_bot: 0.,
            uv_left: 0.,
            uv_width: 0.,
            uv_height: 0.,
        };

        // `RenderableCell` stores viewport coordinates as `usize`.
        let point: Point<usize> = Point::new(id as usize, Column(id as usize));
        ResolvedCell {
            cell: RenderableCell {
                character: 'a',
                point,
                fg: Default::default(),
                bg: Default::default(),
                bg_alpha: 1.,
                underline: Default::default(),
                flags: Flags::empty(),
                extra: None,
            },
            primary: glyph(textures[0]),
            zerowidth: textures[1..].iter().copied().map(glyph).collect(),
        }
    }

    /// Run the production grouping function and return the cell ids in draw order.
    fn draw_order(cells: Vec<ResolvedCell>) -> Vec<i32> {
        let mut cells = cells;
        let mut out = TextureGroupedCells::default();
        group_by_texture(&mut cells, &mut out);
        out.frame_cells().map(|resolved| resolved.cell.point.line as i32).collect()
    }

    #[test]
    fn interleaved_textures_end_up_grouped() {
        // Screen order alternates between two atlas pages, which is what breaks batching.
        let order = draw_order(vec![cell(1, &[1]), cell(2, &[2]), cell(3, &[1]), cell(4, &[2])]);
        assert_eq!(order, vec![1, 3, 2, 4]);
    }

    #[test]
    fn single_texture_stays_in_order() {
        let order = draw_order(vec![cell(1, &[1]), cell(2, &[1]), cell(3, &[1])]);
        assert_eq!(order, vec![1, 2, 3]);
    }

    #[test]
    fn mixed_texture_cells_are_left_ungrouped() {
        // A base glyph plus a combining mark from another atlas page must keep its place
        // after the cells which can be grouped.
        let order = draw_order(vec![cell(7, &[1, 2]), cell(8, &[1])]);
        assert_eq!(order, vec![8, 7]);
    }

    #[test]
    fn bucket_count_bounds_batch_flushes() {
        // Cells spread over 5 atlas pages must yield 5 batches, not one per boundary.
        let mut cells: Vec<_> = (0..50).map(|i| cell(i, &[(i % 5 + 1) as GLuint])).collect();
        let mut out = TextureGroupedCells::default();
        group_by_texture(&mut cells, &mut out);

        assert_eq!(out.buckets.len(), 5);
        assert_eq!(out.ungrouped.len(), 0);
    }

    #[test]
    fn every_glyph_is_emitted_exactly_once() {
        let mut cells: Vec<_> = (0..30).map(|i| cell(i, &[(i * 7 % 4 + 1) as GLuint])).collect();
        let expected = cells.len();
        let mut out = TextureGroupedCells::default();
        group_by_texture(&mut cells, &mut out);

        assert_eq!(out.frame_cells().count(), expected);
    }

    #[test]
    fn combining_marks_stay_attached_to_their_base_glyph() {
        // The mark shares the base glyph's texture, so the cell stays a single unit.
        let mut cells = vec![cell(1, &[1, 1, 1]), cell(2, &[1])];
        let mut out = TextureGroupedCells::default();
        group_by_texture(&mut cells, &mut out);

        let first = out.frame_cells().next().unwrap();
        assert_eq!(first.zerowidth.len(), 2);
    }

    #[test]
    fn buffers_are_reused_across_frames() {
        let mut out = TextureGroupedCells::default();
        let mut cells = vec![cell(1, &[1]), cell(2, &[2])];
        group_by_texture(&mut cells, &mut out);
        let capacity = out.cells.capacity();
        assert!(capacity >= 2);

        // The second frame must reuse the allocation instead of growing it again.
        let mut cells = vec![cell(3, &[1]), cell(4, &[2])];
        group_by_texture(&mut cells, &mut out);
        assert_eq!(out.cells.capacity(), capacity);
        assert_eq!(out.frame_cells().count(), 2);
    }
}
