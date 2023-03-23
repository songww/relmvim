use std::cell::Cell;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::sync::RwLock;

use glib::subclass::prelude::*;

use super::highlights::HighlightDefinitions;

mod imp {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::RwLock;
    use std::sync::RwLockReadGuard;

    use glib::subclass::prelude::*;
    use tracing::{debug, error, trace};

    use crate::vimview::HighlightDefinitions;

    #[derive(debug::Debug)]
    pub struct _TextBuf {
        rows: usize,
        cols: usize,

        #[debug(skip)]
        cells: Box<[super::TextLine]>,

        metrics: Option<Rc<Cell<crate::metrics::Metrics>>>,

        #[debug(skip)]
        hldefs: Option<Rc<RwLock<HighlightDefinitions>>>,

        #[debug(skip)]
        pctx: Option<Rc<pango::Context>>,
    }

    impl Default for _TextBuf {
        fn default() -> Self {
            _TextBuf::new(1, 1)
        }
    }

    impl _TextBuf {
        fn new(rows: usize, cols: usize) -> _TextBuf {
            let cells = _TextBuf::make(rows, cols);
            _TextBuf {
                rows,
                cols,
                cells,
                pctx: None,
                hldefs: None,
                metrics: None,
            }
        }

        fn clear(&mut self) {
            self.cells = _TextBuf::make(self.rows, self.cols);
        }

        fn reset_cache(&mut self) {
            let pctx = self.pctx.as_ref().unwrap();
            let hldefs = self.hldefs.as_ref().unwrap().read().unwrap();
            let metrics = self.metrics.as_ref().unwrap().get();
            self.cells.iter_mut().for_each(|line| {
                line.iter_mut().for_each(|cell| {
                    cell.reset_attrs(pctx, &hldefs, &metrics);
                });
            });
        }

        pub fn set_hldefs(&mut self, hldefs: Rc<RwLock<HighlightDefinitions>>) {
            self.hldefs.replace(hldefs);
        }

        pub fn set_metrics(&mut self, metrics: Rc<Cell<crate::metrics::Metrics>>) {
            self.metrics.replace(metrics);
        }

        pub fn set_pango_context(&mut self, pctx: Rc<pango::Context>) {
            self.pctx.replace(pctx);
        }

        fn set_cells(&mut self, row: usize, col: usize, cells: &[crate::bridge::GridLineCell]) {
            let nrows = self.rows;
            let ncols = self.cols;
            if nrows <= row {
                error!(
                    "set cells dest line {} dose not exists, total {} lines.",
                    row, nrows
                );
                return;
            }
            let line = &self.cells[row];
            line.cache.set(None);
            let pctx = self.pctx.as_ref().unwrap();
            let hldefs = self.hldefs.as_ref().unwrap().read().unwrap();
            let metrics = self.metrics.as_ref().unwrap().get();
            let mut expands = Vec::with_capacity(line.len());
            let mut start_index = line.get(col).map(|cell| cell.start_index).unwrap_or(0);
            for cell in cells.iter() {
                let crate::bridge::GridLineCell {
                    text,
                    hldef,
                    repeat,
                    double_width,
                } = cell;
                for _ in 0..repeat.unwrap_or(1) {
                    // FIXME: invalid start_index
                    let end_index = start_index + text.len();
                    let attrs = Vec::new();
                    let mut cell = super::TextCell {
                        text: text.to_string(),
                        hldef: hldef.clone(),
                        double_width: *double_width,
                        attrs,
                        start_index,
                        end_index,
                    };
                    cell.reset_attrs(pctx, &hldefs, &metrics);
                    trace!(
                        "Setting cell {}x{} start_index {} end_index {}",
                        row,
                        col + expands.len(),
                        start_index,
                        end_index
                    );
                    expands.push(cell);
                    start_index = end_index;
                }
            }
            let col_to = col + expands.len();
            // line.iter()
            //     .enumerate()
            //     .skip(col)
            //     .take(expands.len())
            //     .for_each(|(idx, cell)| {
            //         info!(
            //             "old cell {} start_index {} end_index {}",
            //             idx,
            //             cell.start_index,
            //             cell.end_index
            //         )
            //     });
            debug!(
                "textbuf {}x{} setting line {} with {} cells from {} to {}",
                ncols,
                nrows,
                row,
                expands.len(),
                col,
                col_to
            );
            let line = &mut self.cells[row];
            line[col..col_to].swap_with_slice(&mut expands);
            line.iter_mut().fold(0, |start_index, cell| {
                cell.start_index = start_index;
                cell.end_index = start_index + cell.text.len();
                cell.reset_attrs(pctx, &hldefs, &metrics);
                cell.end_index
            });
        }

        /// drop head of {} rows. leave tail as empty.
        fn up(&mut self, rows: usize) {
            let mut cells = _TextBuf::make(self.rows, self.cols);
            cells[..(self.rows - rows)].swap_with_slice(&mut self.cells[rows..]);
            self.cells = cells;
        }

        /// drop tail of {} rows. leave head as empty.
        fn down(&mut self, rows: usize) {
            let mut cells = _TextBuf::make(self.rows, self.cols);
            cells[rows..].swap_with_slice(&mut self.cells[..(self.rows - rows)]);
            self.cells = cells;
        }

        fn pango_context(&self) -> Rc<pango::Context> {
            self.pctx.clone().unwrap()
        }
    }

    #[derive(Debug, Default)]
    pub struct TextBuf {
        inner: RwLock<_TextBuf>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TextBuf {
        const NAME: &'static str = "TextBuf";
        type Type = super::TextBuf;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for TextBuf {}

    impl TextBuf {
        pub(super) fn up(&self, rows: usize) {
            self.inner.write().unwrap().up(rows);
        }
        pub(super) fn down(&self, rows: usize) {
            self.inner.write().unwrap().down(rows);
        }

        pub(super) fn set_cells(
            &self,
            row: usize,
            col: usize,
            cells: &[crate::bridge::GridLineCell],
        ) {
            self.inner.write().unwrap().set_cells(row, col, cells);
        }

        pub(super) fn set_hldefs(&self, hldefs: Rc<RwLock<HighlightDefinitions>>) {
            self.inner.write().unwrap().set_hldefs(hldefs);
        }

        pub(super) fn set_metrics(&self, metrics: Rc<Cell<crate::metrics::Metrics>>) {
            self.inner.write().unwrap().set_metrics(metrics);
        }

        pub(super) fn set_pango_context(&self, pctx: Rc<pango::Context>) {
            self.inner.write().unwrap().set_pango_context(pctx);
        }

        pub(super) fn pango_context(&self) -> Rc<pango::Context> {
            self.inner.write().unwrap().pango_context()
        }

        pub fn cell(&self, row: usize, col: usize) -> Option<super::TextCell> {
            self.lines()
                .get(row)
                .and_then(|line| line.get(col))
                .cloned()
        }

        pub(super) fn reset_cache(&self) {
            debug!("textbuf rebuild cache");
            self.inner.write().unwrap().reset_cache();
        }

        pub(super) fn clear(&self) {
            debug!("textbuf cleared");
            self.inner.write().unwrap().clear();
        }

        pub(super) fn resize(&self, rows: usize, cols: usize) {
            self.inner.write().unwrap().resize(rows, cols);
        }

        pub(super) fn rows(&self) -> usize {
            self.inner.read().unwrap().rows
        }

        pub(super) fn cols(&self) -> usize {
            self.inner.read().unwrap().cols
        }

        pub(super) fn lines(&self) -> Lines {
            Lines {
                guard: self.inner.read().unwrap(),
            }
        }

        pub(super) fn hldefs(&self) -> Option<Rc<RwLock<HighlightDefinitions>>> {
            self.inner.read().unwrap().hldefs.clone()
        }

        pub(super) fn metrics(&self) -> Option<Rc<Cell<crate::metrics::Metrics>>> {
            self.inner.read().unwrap().metrics.clone()
        }
    }

    trait TextBufExt {
        fn resize(&mut self, rows: usize, cols: usize);

        fn make(rows: usize, cols: usize) -> Box<[super::TextLine]> {
            let tl = super::TextLine::new(cols);
            vec![tl; rows].into_boxed_slice()
        }
    }

    impl TextBufExt for _TextBuf {
        fn resize(&mut self, rows: usize, cols: usize) {
            let old_rows = self.rows;
            let old_cols = self.cols;
            if old_rows == rows && old_cols == cols {
                return;
            }
            self.cols = cols;
            self.rows = rows;
            let nrows = rows.min(old_rows);
            let mut cells = vec![super::TextLine::new(0); rows];
            cells[..nrows].swap_with_slice(&mut self.cells[..nrows]);
            let cells: Vec<_> = cells
                .into_iter()
                .map(|tl| {
                    let mut tl = tl.into_inner().into_vec();
                    let mut start_index = tl.last().map(|last| last.start_index).unwrap_or(0);
                    let old_cols = tl.len();
                    tl.resize(cols, super::TextCell::default());
                    if cols > old_cols {
                        tl.iter_mut().skip(old_cols).for_each(|cell| {
                            cell.start_index = start_index;
                            cell.end_index = start_index + 1;
                            start_index += 1;
                        });
                    }
                    super::TextLine::from(tl.into_boxed_slice())
                })
                .collect();

            debug!(
                "resizing buf cells from {}x{} to {}x{}",
                old_cols, old_rows, cols, rows
            );

            self.cells = cells.into_boxed_slice();
        }
    }

    pub struct Lines<'a> {
        guard: RwLockReadGuard<'a, _TextBuf>,
    }

    impl<'a> Lines<'a> {
        pub fn get(&self, no: usize) -> Option<&super::TextLine> {
            self.guard.cells.get(no)
        }
    }
}

pub use imp::Lines;

glib::wrapper! {
    pub struct TextBuf(ObjectSubclass<imp::TextBuf>);
}

impl TextBuf {
    pub fn new() -> Self {
        glib::Object::new()
    }

    fn imp(&self) -> &imp::TextBuf {
        imp::TextBuf::from_obj(self)
    }

    pub fn clear(&self) {
        self.imp().clear();
    }

    pub fn resize(&self, rows: usize, cols: usize) {
        self.imp().resize(rows, cols);
    }

    pub fn rows(&self) -> usize {
        self.imp().rows()
    }

    pub fn cols(&self) -> usize {
        self.imp().cols()
    }

    pub fn hldefs(&self) -> Option<Rc<RwLock<HighlightDefinitions>>> {
        self.imp().hldefs()
    }

    pub fn metrics(&self) -> Option<Rc<Cell<crate::metrics::Metrics>>> {
        self.imp().metrics()
    }

    pub fn lines(&self) -> Lines {
        self.imp().lines()
    }

    pub fn set_cells(&self, row: usize, col: usize, cells: &[crate::bridge::GridLineCell]) {
        self.imp().set_cells(row, col, cells);
    }

    pub fn set_hldefs(&self, hldefs: Rc<RwLock<HighlightDefinitions>>) {
        self.imp().set_hldefs(hldefs);
    }
    pub fn set_metrics(&self, metrics: Rc<Cell<crate::metrics::Metrics>>) {
        self.imp().set_metrics(metrics);
    }

    pub fn set_pango_context(&self, pctx: Rc<pango::Context>) {
        self.imp().set_pango_context(pctx);
    }

    pub fn pango_context(&self) -> Rc<pango::Context> {
        self.imp().pango_context()
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<TextCell> {
        self.imp().cell(row, col)
    }

    pub fn up(&self, rows: usize) {
        self.imp().up(rows);
    }

    pub fn down(&self, rows: usize) {
        self.imp().down(rows);
    }

    pub fn reset_cache(&self) {
        self.imp().reset_cache();
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextCell {
    pub text: String,
    pub hldef: Option<u64>,
    pub double_width: bool,
    pub attrs: Vec<pango::Attribute>,
    pub start_index: usize,
    pub end_index: usize,
}

impl Default for TextCell {
    fn default() -> TextCell {
        TextCell {
            text: String::from(" "),
            hldef: None,
            double_width: false,
            attrs: Vec::new(),
            start_index: 0,
            end_index: 0,
        }
    }
}

impl TextCell {
    fn reset_attrs(
        &mut self,
        _pctx: &pango::Context,
        hldefs: &HighlightDefinitions,
        _metrics: &crate::metrics::Metrics,
    ) {
        const U16MAX: f32 = u16::MAX as f32;

        self.attrs.clear();
        let attrs = pango::AttrList::new();

        if self.end_index == self.start_index {
            return;
        }

        let start_index = self.start_index as u32;
        let end_index = self.end_index as u32;
        let default_hldef = hldefs.get(HighlightDefinitions::DEFAULT).unwrap();
        let default_colors = hldefs.defaults().unwrap();
        let mut background = None;
        let mut hldef = default_hldef;
        if let Some(ref id) = self.hldef {
            let style = hldefs.get(*id);
            if let Some(style) = style {
                background = style.background();
                hldef = style;
            }
        }
        if hldef.italic {
            let mut attr = pango::AttrInt::new_style(pango::Style::Italic);
            attr.set_start_index(start_index);
            attr.set_end_index(end_index);
            attrs.insert(attr);
        }
        if hldef.bold {
            let mut attr = pango::AttrInt::new_weight(pango::Weight::Semibold);
            attr.set_start_index(start_index);
            attr.set_end_index(end_index);
            attrs.insert(attr);
        }
        if hldef.strikethrough {
            let mut attr = pango::AttrInt::new_strikethrough(true);
            attr.set_start_index(start_index);
            attr.set_end_index(end_index);
            attrs.insert(attr);
        }
        if hldef.underline {
            let mut attr = pango::AttrInt::new_underline(pango::Underline::Single);
            attr.set_start_index(start_index);
            attr.set_end_index(end_index);
            attrs.insert(attr);
        }
        if hldef.undercurl {
            let mut attr = pango::AttrInt::new_underline(pango::Underline::Error);
            attr.set_start_index(start_index);
            attr.set_end_index(end_index);
            attrs.insert(attr);
        }
        // alpha color
        // blend is 0 - 100. Could be used by UIs to support
        // blending floating windows to the background or to
        // signal a transparent cursor.
        // let blend = u16::MAX as u32 * hldef.blend as u32 / 100;
        // let mut attr = pango::AttrInt::new_background_alpha(blend as u16);
        // info!("blend {}", hldef.blend);
        // attr.set_start_index(start_index as _);
        // attr.set_end_index(end_index as _);
        // attrs.insert(attr);
        if let Some(fg) = hldef.colors.foreground.or(default_colors.foreground) {
            let mut attr = pango::AttrColor::new_foreground(
                (fg.red() * U16MAX).round() as u16,
                (fg.green() * U16MAX).round() as u16,
                (fg.blue() * U16MAX).round() as u16,
            );
            attr.set_start_index(start_index);
            attr.set_end_index(end_index);
            attrs.insert(attr);
        }
        if let Some(bg) = background {
            let mut attr = pango::AttrColor::new_background(
                (bg.red() * U16MAX).round() as u16,
                (bg.green() * U16MAX).round() as u16,
                (bg.blue() * U16MAX).round() as u16,
            );
            attr.set_start_index(start_index);
            attr.set_end_index(end_index);
            attrs.insert(attr);
        }
        if let Some(special) = hldef.colors.special.or(default_colors.special) {
            let mut attr = pango::AttrColor::new_underline_color(
                (special.red() * U16MAX).round() as u16,
                (special.green() * U16MAX).round() as u16,
                (special.blue() * U16MAX).round() as u16,
            );
            attr.set_start_index(start_index);
            attr.set_end_index(end_index);
            attrs.insert(attr);
        }

        self.attrs = attrs.attributes();
    }
}

#[derive(Default, debug::Debug)]
pub struct TextLine {
    #[debug(skip)]
    boxed: Box<[TextCell]>,
    #[debug(skip)]
    cache: Cell<Option<(pango::Layout, pango::LayoutLine)>>,
}

impl Clone for TextLine {
    fn clone(&self) -> Self {
        TextLine {
            boxed: self.boxed.clone(),
            cache: Cell::new(unsafe { &*self.cache.as_ptr() }.clone()),
        }
    }
}

impl TextLine {
    fn new(cols: usize) -> TextLine {
        let mut line = Vec::with_capacity(cols);
        line.resize(cols, TextCell::default());
        line.iter_mut().enumerate().for_each(|(start_index, cell)| {
            cell.start_index = start_index;
            cell.end_index = start_index + 1;
        });
        Self {
            boxed: line.into_boxed_slice(),
            cache: Cell::new(None),
        }
    }

    pub fn cache(&self) -> Option<(pango::Layout, pango::LayoutLine)> {
        unsafe { &*self.cache.as_ptr() }.clone()
    }

    pub fn set_cache(&self, layout: pango::Layout, line: pango::LayoutLine) {
        self.cache.set((layout, line).into());
    }
}

impl Deref for TextLine {
    type Target = [TextCell];

    fn deref(&self) -> &[TextCell] {
        &self.boxed
    }
}

impl DerefMut for TextLine {
    fn deref_mut(&mut self) -> &mut [TextCell] {
        &mut self.boxed
    }
}

impl AsRef<[TextCell]> for TextLine {
    fn as_ref(&self) -> &[TextCell] {
        &self.boxed
    }
}

impl AsMut<[TextCell]> for TextLine {
    fn as_mut(&mut self) -> &mut [TextCell] {
        &mut self.boxed
    }
}

impl From<Box<[TextCell]>> for TextLine {
    fn from(boxed: Box<[TextCell]>) -> Self {
        TextLine {
            boxed,
            ..Default::default()
        }
    }
}

impl Into<Box<[TextCell]>> for TextLine {
    fn into(self) -> Box<[TextCell]> {
        self.boxed
    }
}

impl TextLine {
    fn into_inner(self) -> Box<[TextCell]> {
        self.boxed
    }
}
