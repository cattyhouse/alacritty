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
            // Resolve every glyph up front and group the cells by the atlas texture they ended
            // up in. Without this the batch is flushed at every texture boundary, because screen
            // traversal order (row by row) does not match the order glyphs were allocated into
            // atlas pages. With a lot of CJK text that turns a frame into dozens of tiny draws.
            //
            // Grouping is safe because cells never overlap on screen, so the draw order within
            // a frame is not observable.
            let mut buckets: Vec<GlyphBucket> = Vec::with_capacity(8);
            let mut ungrouped: Vec<ResolvedCell> = Vec::new();

            for cell in cells {
                let resolved = api.resolve_cell(cell, glyph_cache);

                // Only group cells whose glyphs all live in the same texture. Anything mixed
                // (for example a base glyph plus zero-width combining marks from another atlas
                // page) keeps its original order.
                match resolved.single_texture() {
                    Some(tex_id) => {
                        match buckets.iter_mut().find(|bucket| bucket.tex_id == tex_id) {
                            Some(bucket) => bucket.entries.push(resolved),
                            None => {
                                let mut bucket =
                                    GlyphBucket { tex_id, entries: Vec::with_capacity(256) };
                                bucket.entries.push(resolved);
                                buckets.push(bucket);
                            },
                        }
                    },
                    None => ungrouped.push(resolved),
                }
            }

            let mut emit = |resolved: &ResolvedCell| {
                api.add_render_item(&resolved.cell, &resolved.primary, size_info);
                for glyph in &resolved.zerowidth {
                    api.add_render_item(&resolved.cell, glyph, size_info);
                }
            };

            for bucket in &buckets {
                for resolved in &bucket.entries {
                    emit(resolved);
                }
            }
            for resolved in &ungrouped {
                emit(resolved);
            }
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
        if self.zerowidth.is_empty() || self.zerowidth.iter().all(|g| g.tex_id == self.primary.tex_id)
        {
            Some(self.primary.tex_id)
        } else {
            None
        }
    }
}

/// Cells whose glyphs all live in one atlas texture.
struct GlyphBucket {
    tex_id: GLuint,
    entries: Vec<ResolvedCell>,
}

#[cfg(test)]
struct BucketIds {
    tex_id: u32,
    ids: Vec<Vec<u32>>,
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
    use super::BucketIds as GlyphBucket;

    /// A resolved cell: its glyphs paired with the texture each glyph lives in.
    type Resolved = Vec<(u32, u32)>;

    /// Mirrors the grouping rule in `draw_cells` exactly, without needing a GL context.
    fn group(entries: &[Resolved]) -> (Vec<(u32, Vec<Vec<u32>>)>, Vec<Resolved>) {
        let mut buckets: Vec<GlyphBucket> = Vec::new();
        let mut ungrouped: Vec<Resolved> = Vec::new();

        for glyphs in entries {
            let cell_tex = glyphs[0].1;
            match glyphs.first() {
                Some(first) if glyphs.iter().all(|(_, tex)| *tex == first.1) => {
                    match buckets.iter_mut().find(|bucket| bucket.tex_id == cell_tex) {
                        Some(bucket) => bucket.ids.push(glyphs.iter().map(|(id, _)| *id).collect()),
                        None => buckets.push(GlyphBucket {
                            tex_id: cell_tex,
                            ids: vec![glyphs.iter().map(|(id, _)| *id).collect()],
                        }),
                    }
                },
                _ => ungrouped.push(glyphs.clone()),
            }
        }

        let grouped = buckets.into_iter().map(|b| (b.tex_id, b.ids)).collect();
        (grouped, ungrouped)
    }

    fn flat(groups: &[(u32, Vec<Vec<u32>>)]) -> Vec<Vec<u32>> {
        groups.iter().flat_map(|(_, ids)| ids.clone()).collect()
    }

    fn g(id: u32, tex: u32) -> Resolved {
        vec![(id, tex)]
    }

    #[test]
    fn interleaved_textures_end_up_grouped() {
        // Screen order alternates between two atlas pages, which is what breaks batching.
        let (buckets, ungrouped) = group(&[g(1, 1), g(2, 2), g(3, 1), g(4, 2), g(5, 1)]);

        assert!(ungrouped.is_empty());
        assert_eq!(flat(&buckets), vec![vec![1], vec![3], vec![5], vec![2], vec![4]]);
    }

    #[test]
    fn single_texture_stays_in_order() {
        let (buckets, ungrouped) = group(&[g(1, 1), g(2, 1), g(3, 1)]);

        assert!(ungrouped.is_empty());
        assert_eq!(flat(&buckets), vec![vec![1], vec![2], vec![3]]);
    }

    #[test]
    fn mixed_texture_cells_are_left_ungrouped() {
        // A base glyph plus a combining mark from another atlas page must keep its place.
        let (buckets, ungrouped) = group(&[vec![(7, 1), (9, 2)], g(8, 1)]);

        assert_eq!(flat(&buckets), vec![vec![8]]);
        assert_eq!(ids(&ungrouped), vec![vec![7, 9]]);
    }

    fn ids(resolved: &[Resolved]) -> Vec<Vec<u32>> {
        resolved.iter().map(|e| e.iter().map(|(id, _)| *id).collect()).collect()
    }

    #[test]
    fn bucket_count_bounds_batch_flushes() {
        // 50 cells spread over 5 atlas pages must yield exactly 5 batches, not 50.
        let entries: Vec<Resolved> = (0..50).map(|i| g(i, (i % 5) + 1)).collect();

        let (buckets, _) = group(&entries);

        // Exactly one bucket per atlas page, regardless of how the cells interleaved.
        assert_eq!(buckets.len(), 5);
        let distinct: std::collections::BTreeSet<u32> =
            buckets.iter().map(|(tex, _)| *tex).collect();
        assert_eq!(distinct, [1, 2, 3, 4, 5].into_iter().collect());
        assert_eq!(buckets.iter().map(|(_, ids)| ids.len()).sum::<usize>(), 50);
    }

    #[test]
    fn every_glyph_is_emitted_exactly_once() {
        let entries: Vec<Resolved> = (0..30).map(|i| g(i, (i * 7) % 4 + 1)).collect();
        let expected: usize = entries.iter().map(|e| e.len()).sum();

        let (buckets, ungrouped) = group(&entries);
        let emitted: usize =
            buckets.iter().map(|(_, ids)| ids.len()).sum::<usize>() + ungrouped.len();

        assert_eq!(emitted, expected);
    }

    #[test]
    fn combining_marks_stay_attached_to_their_base_glyph() {
        let (buckets, ungrouped) = group(&[vec![(1, 1), (2, 1), (3, 1)], g(4, 1)]);

        assert!(ungrouped.is_empty());
        // The base glyph and its marks must remain adjacent and in order.
        assert_eq!(flat(&buckets), vec![vec![1, 2, 3], vec![4]]);
    }
}
