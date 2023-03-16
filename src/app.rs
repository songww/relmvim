use std::cell::{Cell, RefCell};
use std::convert::identity;
use std::rc::Rc;
use std::sync::{atomic, Arc, RwLock};

use gtk::gdk;
use gtk::gdk::prelude::FontMapExt;
use gtk::gdk::ScrollDirection;
use gtk::prelude::*;

use adw::prelude::*;
use once_cell::sync::{Lazy, OnceCell};
use pango::FontDescription;
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;
use rustc_hash::FxHashMap;
use tracing::{debug, error, info, trace, warn};

use crate::bridge;
use crate::bridge::{
    EditorMode, MouseButton, ParallelCommand, RedrawEvent, SerialCommand, UiCommand, WindowAnchor,
};
use crate::components::{VimCmdEvent, VimCmdPrompts};
use crate::cursor::{CursorMode, VimCursor};
use crate::event_aggregator::EVENT_AGGREGATOR;
use crate::grapheme::Coord;
use crate::keys::ToInput;
use crate::metrics::Metrics;
use crate::vimview::{self, VimGrid, VimMessage};
use crate::widgets::board::Board;
use crate::Opts;

#[allow(non_upper_case_globals)]
pub static GridActived: Lazy<Arc<atomic::AtomicU64>> =
    Lazy::new(|| Arc::new(atomic::AtomicU64::new(0)));

#[derive(Clone, Debug)]
pub enum AppMessage {
    Quit,
    ShowPointer,
    UiCommand(UiCommand),
    RedrawEvent(RedrawEvent),
}

impl From<UiCommand> for AppMessage {
    fn from(cmd: UiCommand) -> Self {
        AppMessage::UiCommand(cmd)
    }
}

#[derive(Debug)]
pub struct AppModel {
    pub opts: Opts,

    pub title: String,
    pub size: Rc<Cell<(i32, i32)>>,
    pub default_width: i32,
    pub default_height: i32,

    pub guifont: Option<String>,
    pub guifontset: Option<String>,
    pub guifontwide: Option<String>,
    pub metrics: Rc<Cell<Metrics>>,
    pub show_tab_line: Option<u64>,

    pub font_description: Rc<RefCell<pango::FontDescription>>,
    pub font_changed: Rc<atomic::AtomicBool>,

    pub mode: EditorMode,

    pub mouse_on: Rc<atomic::AtomicBool>,
    // pub cursor: Component<VimCursor>,
    pub cursor_grid: u64,
    pub cursor_coord: Coord,
    pub cursor_coord_changed: atomic::AtomicBool,
    pub cursor_mode: usize,
    pub cursor_modes: Vec<CursorMode>,

    cmd_prompt: Controller<VimCmdPrompts>,

    pub pctx: Rc<pango::Context>,
    pub gtksettings: OnceCell<gtk::Settings>,
    pub im_context: OnceCell<gtk::IMMulticontext>,

    pub hldefs: Rc<RwLock<vimview::HighlightDefinitions>>,
    pub hlgroups: Rc<RwLock<FxHashMap<String, u64>>>,

    pub background_changed: Rc<atomic::AtomicBool>,

    pub vgrids: crate::factory::Factory<vimview::VimGrid>,
    pub messages: FactoryVecDeque<vimview::VimMessage>,
    pub dragging: Rc<Cell<Option<Dragging>>>,
    pub show_pointer: atomic::AtomicBool,

    pub rt: tokio::runtime::Runtime,
}

#[derive(Clone, Copy, Debug)]
pub struct Dragging {
    pub btn: MouseButton,
    pub pos: (u32, u32),
}

impl AppModel {
    pub fn calculate(&self) {
        const PANGO_SCALE: f64 = pango::SCALE as f64;
        const SINGLE_WIDTH_CHARS: &'static str = concat!(
            " ! \" # $ % & ' ( ) * + , - . / ",
            "0 1 2 3 4 5 6 7 8 9 ",
            ": ; < = > ? @ ",
            "A B C D E F G H I J K L M N O P Q R S T U V W X Y Z ",
            "[ \\ ] ^ _ ` ",
            "a b c d e f g h i j k l m n o p q r s t u v w x y z ",
            "{ | } ~ ",
            ""
        );
        let desc = self.font_description.borrow_mut();
        debug!(
            "font desc {} {} {} {}",
            desc.family().unwrap(),
            desc.weight(),
            desc.style(),
            desc.size() / pango::SCALE,
        );
        let layout = pango::Layout::new(&self.pctx);
        layout.set_font_description(Some(&desc));
        let mut tabs = pango::TabArray::new(1, false);
        tabs.set_tab(0, pango::TabAlign::Left, 1);
        layout.set_tabs(Some(&tabs));
        let mut max_width = 1;
        let mut max_height = 1;

        (0x21u8..0x7f).for_each(|c| {
            let text = unsafe { String::from_utf8_unchecked(vec![c]) };
            layout.set_text(&text);
            let (_ink, logical) = layout.extents();
            max_height = logical.height().max(max_height);
            max_width = logical.width().max(max_width);
        });

        layout.set_text(SINGLE_WIDTH_CHARS);
        let ascent = layout.baseline() as f64 / PANGO_SCALE;
        let font_metrics = self.pctx.metrics(Some(&desc), None);
        let fm_width = font_metrics.approximate_digit_width();
        let fm_height = font_metrics.height();
        let fm_ascent = font_metrics.ascent();
        info!("font-metrics width: {}", fm_width as f64 / PANGO_SCALE);
        info!("font-metrics height: {}", fm_height as f64 / PANGO_SCALE);
        info!("font-metrics ascent: {}", fm_ascent as f64 / PANGO_SCALE);
        let mut metrics = self.metrics.get();
        let charwidth = max_width as f64 / PANGO_SCALE;
        let width = charwidth;
        let charheight = if fm_height > 0 {
            fm_height.min(max_height) as f64 / PANGO_SCALE
        } else {
            max_height as f64 / PANGO_SCALE
        };
        if metrics.charheight() == charheight
            && metrics.charwidth() == charwidth
            && metrics.width() == width
        {
            return;
        }
        metrics.set_width(width.ceil());
        metrics.set_ascent(ascent.ceil());
        metrics.set_charwidth(charwidth.ceil());
        metrics.set_charheight(charheight.ceil());
        info!("char-width {:?}", metrics.charwidth());
        info!("char-height {:?}", metrics.charheight());
        info!("char-ascent {:?}", metrics.ascent());
        self.metrics.replace(metrics);
    }
}

#[relm4::component]
impl Component for AppModel {
    type Init = Opts;

    type Input = AppMessage;
    type Output = AppMessage;
    type CommandOutput = ();

    view! {
        main_window = gtk::ApplicationWindow {
            set_default_width: model.default_width,
            set_default_height: model.default_height,
            set_cursor_from_name: Some("text"),
            #[watch]
            set_title: Some(&model.title),
            #[name(vbox)]
            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,
                set_spacing: 0,
                set_hexpand: true,
                set_vexpand: true,
                set_focusable: true,
                set_sensitive: true,
                set_can_focus: true,
                set_can_target: true,
                set_focus_on_click: true,

                // set_child: Add tabline

                append: overlay = &gtk::Overlay {
                    set_focusable: true,
                    set_sensitive: true,
                    set_can_focus: true,
                    set_can_target: true,
                    set_focus_on_click: true,
                    #[wrap(Some)]
                    set_child: da = &gtk::DrawingArea {
                        set_hexpand: true,
                        set_vexpand: true,
                        set_focus_on_click: false,
                        set_overflow: gtk::Overflow::Hidden,
                        connect_resize[sender = sender.clone(), metrics = model.metrics.clone(), size = model.size.clone()] => move |da, width, height| {
                            debug!("da resizing width: {}, height: {}", width, height);
                            size.set((width, height));
                            let metrics = metrics.get();
                            let rows = da.height() as f64 / metrics.height(); //  + metrics.linespace
                            let cols = da.width() as f64 / metrics.width();
                            debug!("da resizing rows: {} cols: {}", rows, cols);
                            sender
                                .output(
                                    UiCommand::Parallel(ParallelCommand::Resize {
                                        width: cols as _,
                                        height: rows as _,
                                    })
                                    .into(),
                                )
                                .unwrap();
                        },
                        set_draw_func[hldefs = model.hldefs.clone()] => move |_da, cr, w, h| {
                            let hldefs = hldefs.read().unwrap();
                            let default_colors = hldefs.defaults().unwrap();
                            debug!("drawing default background {}x{}.", w, h);
                            if let Some(bg) = default_colors.background {
                                cr.rectangle(0., 0., w.into(), h.into());
                                cr.set_source_rgb(bg.red() as _, bg.green() as _, bg.blue() as _);
                                cr.paint().unwrap();
                            }
                        }
                    },
                    #[name(grids_container)]
                    add_overlay = &Board {
                        set_widget_name: "grids-container",
                        set_visible: true,
                        set_focus_on_click: true,

                        #[local_ref]
                        vgrids -> gtk::Box {
                            //
                        }
                    },
                    add_overlay: float_win_container = &gtk::Fixed {
                        set_widget_name: "float-win-container",
                        set_visible: false,
                        set_hexpand: false,
                        set_vexpand: false,
                    },
                    // add_overlay: model.cursor.root_widget(),
                    add_overlay: messages_container = &gtk::Box {
                        set_widget_name: "messages-container",
                        set_opacity: 0.95,
                        set_spacing: 5,
                        set_visible: false,
                        set_hexpand: true,
                        // It dosenot matter.
                        set_width_request: 0,
                        set_homogeneous: false,
                        set_focus_on_click: false,
                        set_halign: gtk::Align::End,
                        set_valign: gtk::Align::Start,
                        set_overflow: gtk::Overflow::Visible,
                        set_orientation: gtk::Orientation::Vertical,
                        #[local_ref]
                        messages -> gtk::Box {
                            //
                        }
                    },
                    // add_overlay: components.cmd_prompt.root_widget() ,
                }
            },
            connect_close_request[sender = sender.clone()] => move |_| {
                sender.output(AppMessage::UiCommand(UiCommand::Parallel(ParallelCommand::Quit))).ok();
                gtk::Inhibit(true)
            },
        }
    }

    additional_fields! {
        pointer_animation: adw::TimedAnimation,
    }

    fn pre_view() {
        if let Ok(true) = model.show_pointer.compare_exchange(
            true,
            false,
            atomic::Ordering::Acquire,
            atomic::Ordering::Relaxed,
        ) {
            pointer_animation.play();
        }
        if let Ok(true) = model.background_changed.compare_exchange(
            true,
            false,
            atomic::Ordering::Acquire,
            atomic::Ordering::Relaxed,
        ) {
            da.queue_draw();
        }
        if let Ok(true) = model.cursor_coord_changed.compare_exchange(
            true,
            false,
            atomic::Ordering::Acquire,
            atomic::Ordering::Relaxed,
        ) {
            let coord = &model.cursor_coord;
            let metrics = model.metrics.get();
            if let Some(base) = model.vgrids.get(model.cursor_grid).map(|vg| vg.coord()) {
                let (col, row) = (base.col + coord.col, base.row + coord.row);
                let (x, y) = (col * metrics.width(), row * metrics.height());
                let rect = gdk::Rectangle::new(
                    x as i32,
                    y as i32,
                    metrics.width() as i32,
                    metrics.height() as i32,
                );
                unsafe { model.im_context.get_unchecked() }.set_cursor_location(&rect);
            }
        }
        if let Ok(true) = model.font_changed.compare_exchange(
            true,
            false,
            atomic::Ordering::Acquire,
            atomic::Ordering::Relaxed,
        ) {
            debug!(
                "default font name: {}",
                model.font_description.borrow().to_str()
            );
            let (width, height) = model.size.get();
            let metrics = model.metrics.get();
            let rows = height as f64 / metrics.height();
            let cols = width as f64 / metrics.width();
            info!(
                "trying resize nvim to {}x{} original {}x{} {:?}",
                rows, cols, width, height, metrics
            );
            sender
                .output(
                    UiCommand::Parallel(ParallelCommand::Resize {
                        width: cols as _,
                        height: rows as _,
                    })
                    .into(),
                )
                .unwrap();
        }
    }

    fn post_view() {
        self.calculate();
        self.gtksettings.set(widgets.overlay.settings()).ok();
        let metrics = self.metrics.get();
        let rows = (self.opts.height as f64 / metrics.height()).ceil() as i64;
        let cols = (self.opts.width as f64 / metrics.width()).ceil() as i64;
        let mut opts = self.opts.clone();
        opts.size.replace((cols, rows));
        self.rt.spawn(bridge::open(opts));
        widgets.da.queue_allocate();
        widgets.da.queue_resize();
        widgets.da.queue_draw();

        pointer_animation.set_easing(adw::Easing::Linear);
        pointer_animation.set_repeat_count(1);
        pointer_animation.connect_done(move |this| {
            this.widget().set_cursor_from_name(Some("none"));
        });

        let im_context = gtk::IMMulticontext::new();
        im_context.set_use_preedit(false);
        im_context.set_client_widget(Some(&widgets.overlay));

        im_context.set_input_purpose(gtk::InputPurpose::Terminal);

        im_context.set_cursor_location(&gdk::Rectangle::new(0, 0, 5, 10));
        // im_context.connect_preedit_start(|_| {
        //     debug!("preedit started.");
        // });
        // im_context.connect_preedit_end(|im_context| {
        //     debug!("preedit done, '{}'", im_context.preedit_string().0);
        // });
        // im_context.connect_preedit_changed(|im_context| {
        //     debug!("preedit changed, '{}'", im_context.preedit_string().0);
        // });

        im_context.connect_commit(gtk::glib::clone!(@strong sender => move |ctx, text| {
            debug!("im-context({}) commit '{}'", ctx.context_id(), text);
            sender
                .output(UiCommand::Serial(SerialCommand::Keyboard(text.replace("<", "<lt>").into())).into());
        }));

        main_window.set_focus_widget(Some(&widgets.overlay));
        main_window.set_default_widget(Some(&widgets.overlay));

        let grids_container = widgets.grids_container;

        let listener = gtk::EventControllerScroll::builder()
            .flags(gtk::EventControllerScrollFlags::all())
            .name("vimview-scrolling-listener")
            .build();
        listener.connect_scroll(glib::clone!(@strong sender, @strong self.mouse_on as mouse_on, @strong grids_container => move |c, x, y| {
            if !mouse_on.load(atomic::Ordering::Relaxed) {
                return gtk::Inhibit(false)
            }
            let event = c.current_event().unwrap().downcast::<gdk::ScrollEvent>().unwrap();
            let modifier = event.modifier_state();
            let id = GridActived.load(atomic::Ordering::Relaxed);
            let direction = match event.direction() {
                ScrollDirection::Up => {
                    "up"
                },
                    ScrollDirection::Down => {
                    "down"
                }
                ScrollDirection::Left => {
                    "left"
                }
                ScrollDirection::Right => {
                    "right"
                }
                _ => {
                    return gtk::Inhibit(false)
                }
            };
            debug!("scrolling grid {} x: {}, y: {} {}", id, x, y, &direction);
            let command = UiCommand::Serial(SerialCommand::Scroll { direction: direction.into(), grid_id: id, position: (0, 1), modifier });
            sender.output(AppMessage::UiCommand(command));
            gtk::Inhibit(false)
        }));

        main_window.add_controller(listener);

        let focus_controller = gtk::EventControllerFocus::builder()
            .name("vimview-focus-controller")
            .build();
        focus_controller.connect_enter(
            glib::clone!(@strong sender, @strong im_context => move |_| {
                info!("FocusGained");
                im_context.focus_in();
                sender.output(UiCommand::Parallel(ParallelCommand::FocusGained).into());
            }),
        );
        focus_controller.connect_leave(
            glib::clone!(@strong sender, @strong im_context  => move |_| {
                info!("FocusLost");
                im_context.focus_out();
                sender.output(UiCommand::Parallel(ParallelCommand::FocusLost).into());
            }),
        );
        main_window.add_controller(focus_controller);

        let key_controller = gtk::EventControllerKey::builder()
            .name("vimview-key-controller")
            .build();
        key_controller.set_im_context(Some(&im_context));
        key_controller.connect_key_pressed(
            glib::clone!(@strong sender => move |c, keyval, _keycode, modifier| {
                let event = c.current_event().unwrap();

                if let Some(true) = c.im_context().map(|imctx|imctx.filter_keypress(&event)) {
                    debug!("keypress handled by im-context.");
                    return gtk::Inhibit(true)
                }
                let keypress = (keyval, modifier);
                debug!("keypress : {:?}", keypress);
                if let Some(keypress) = keypress.to_input() {
                    debug!("keypress {} sent to neovim.", keypress);
                    sender.output(UiCommand::Serial(SerialCommand::Keyboard(keypress)).into()).unwrap();
                    gtk::Inhibit(true)
                } else {
                    info!("keypress ignored: {:?}", keyval.name());
                    gtk::Inhibit(false)
                }
            }),
        );
        widgets.overlay.add_controller(key_controller);
        self.im_context.set(im_context).unwrap();
    }

    fn init(
        opts: Opts,
        main_window: &Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<AppModel> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_time()
            .enable_io()
            .build()
            .unwrap();
        let font_desc = FontDescription::from_string("monospace 11");
        let size = Rc::new(Cell::new((opts.width, opts.height)));
        let pctx: Rc<pango::Context> = {
            let font_map = pangocairo::FontMap::default();
            let ctx = font_map.create_context();
            // ctx.set_round_glyph_positions(true);
            ctx.set_font_description(Some(&font_desc));
            ctx.set_base_dir(pango::Direction::Ltr);
            ctx.set_language(Some(&pango::Language::from_string("en-US")));
            let mut options = cairo::FontOptions::new().ok();
            options.as_mut().map(|options| {
                // options.set_hint_style(cairo::HintStyle::Full);
                // options.set_antialias(cairo::Antialias::Subpixel);
                options.set_hint_metrics(cairo::HintMetrics::On);
            });
            pangocairo::context_set_font_options(&ctx, options.as_ref());
            ctx.into()
        };
        let hldefs = Rc::new(RwLock::new(vimview::HighlightDefinitions::new()));
        let metrics = Rc::new(Metrics::new().into());

        let mut model = AppModel {
            size,
            title: opts.title.clone(),
            default_width: opts.width,
            default_height: opts.height,
            guifont: None,
            guifontset: None,
            guifontwide: None,
            show_tab_line: None,

            mode: EditorMode::Normal,

            mouse_on: Rc::new(false.into()),
            // cursor: MicroComponent::new(
            //     VimCursor::new(pctx.clone(), Rc::clone(&metrics), hldefs.clone()),
            //     (),
            // ),
            cursor_grid: 0,
            cursor_mode: 0,
            cursor_modes: Vec::new(),
            cursor_coord: Coord::default(),
            cursor_coord_changed: atomic::AtomicBool::new(false),

            pctx,
            gtksettings: OnceCell::new(),
            im_context: OnceCell::new(),

            metrics,
            font_description: Rc::new(RefCell::new(font_desc)),
            font_changed: Rc::new(false.into()),

            hldefs: hldefs.clone(),
            hlgroups: Rc::new(RwLock::new(FxHashMap::default())),

            background_changed: Rc::new(false.into()),

            vgrids: crate::factory::Factory::new(main_window, sender.input_sender()),
            messages: FactoryVecDeque::new(main_window, sender.input_sender()),
            cmd_prompt: VimCmdPrompts::builder()
                // .transient_for(main_window)
                .launch(hldefs.clone())
                .forward(sender.input_sender(), identity),
            dragging: Rc::new(Cell::new(None)),
            show_pointer: true.into(),

            opts,

            rt,
        };

        let vgrids = model.vgrids.widget();
        let messages = model.messages.widget();

        let target =
            adw::CallbackAnimationTarget::new(glib::clone!(@weak main_window => move |_| {
                main_window.set_cursor_from_name(Some("text"));
            }));

        let pointer_animation = adw::TimedAnimation::new(vgrids, 0., 1., 1000, target);

        let widgets = view_output!();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, message: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match message {
            AppMessage::UiCommand(ui_command) => {
                trace!("ui-commad {:?}", ui_command);
                EVENT_AGGREGATOR.send(ui_command);
            }
            AppMessage::Quit => {}
            AppMessage::ShowPointer => {
                self.show_pointer.store(true, atomic::Ordering::Relaxed);
            }
            AppMessage::RedrawEvent(event) => {
                match event {
                    RedrawEvent::SetTitle { title } => {
                        self.title = title
                            .split("     ")
                            .filter_map(|s| if s.is_empty() { None } else { Some(s.trim()) })
                            .collect::<Vec<_>>()
                            .join("  ")
                    }
                    RedrawEvent::OptionSet { gui_option } => match gui_option {
                        bridge::GuiOption::AmbiWidth(ambi_width) => {
                            debug!("unhandled ambi_width {}", ambi_width);
                        }
                        bridge::GuiOption::ArabicShape(arabic_shape) => {
                            debug!("unhandled arabic-shape: {}", arabic_shape);
                        }
                        bridge::GuiOption::Emoji(emoji) => {
                            debug!("emoji: {}", emoji);
                        }
                        bridge::GuiOption::GuiFont(guifont) => {
                            if !guifont.trim().is_empty() {
                                info!("gui font: {}", &guifont);
                                let desc = pango::FontDescription::from_string(
                                    &guifont.replace(":h", " "),
                                );

                                self.pctx.set_font_description(Some(&desc));
                                self.gtksettings.get().map(|settings| {
                                    settings.set_gtk_font_name(Some(&desc.to_str()));
                                });

                                self.guifont.replace(guifont);
                                self.font_description.replace(desc);

                                self.calculate();

                                self.vgrids
                                    .iter_mut()
                                    .for_each(|(_, vgrid)| vgrid.reset_cache());

                                self.font_changed.store(true, atomic::Ordering::Relaxed);
                                self.cursor_coord_changed
                                    .store(true, atomic::Ordering::Relaxed);
                            }
                        }
                        bridge::GuiOption::GuiFontSet(guifontset) => {
                            self.guifontset.replace(guifontset);
                        }
                        bridge::GuiOption::GuiFontWide(guifontwide) => {
                            self.guifontwide.replace(guifontwide);
                        }
                        bridge::GuiOption::LineSpace(linespace) => {
                            info!("line space: {}", linespace);
                            let mut metrics = self.metrics.get();
                            metrics.set_linespace(linespace as _);
                            self.metrics.replace(metrics);
                        }
                        bridge::GuiOption::ShowTabLine(show_tab_line) => {
                            self.show_tab_line.replace(show_tab_line);
                        }
                        bridge::GuiOption::TermGuiColors(term_gui_colors) => {
                            debug!("unhandled term gui colors: {}", term_gui_colors);
                        }
                        bridge::GuiOption::Pumblend(pumblend) => {
                            debug!("unhandled pumblend: {}", pumblend)
                        }
                        bridge::GuiOption::Unknown(name, value) => {
                            debug!("GuiOption({}: {:?}) not supported yet.", name, value)
                        }
                    },
                    RedrawEvent::DefaultColorsSet { colors } => {
                        self.background_changed
                            .store(true, atomic::Ordering::Relaxed);
                        self.hldefs.write().unwrap().set_defaults(colors);
                    }
                    RedrawEvent::HighlightAttributesDefine { id, style } => {
                        self.hldefs.write().unwrap().set(id, style);
                    }
                    RedrawEvent::HighlightGroupSet { name, id } => {
                        self.hlgroups.write().unwrap().insert(name, id);
                        trace!(
                            "current highlight groups: {:?}",
                            self.hlgroups.read().unwrap()
                        );
                    }
                    RedrawEvent::Clear { grid } => {
                        debug!("cleared grid {}", grid);
                        self.vgrids.get_mut(grid).map(|grid| grid.clear());
                    }
                    RedrawEvent::GridLine {
                        grid,
                        row,
                        column_start,
                        cells,
                    } => {
                        debug!(
                            "grid {} line - {} cells at {}x{}",
                            grid,
                            cells.len(),
                            row,
                            column_start
                        );

                        let grids: Vec<_> = self.vgrids.iter().map(|(k, _)| k).collect();
                        let vgrid = self.vgrids.get_mut(grid).expect(&format!(
                            "grid {} not found, valid grids {:?}",
                            grid, &grids
                        ));
                        vgrid
                            .textbuf()
                            .borrow()
                            .set_cells(row as _, column_start as _, &cells);
                        let row = row as usize;
                        let coord = &self.cursor_coord;
                        let cursor_grid = self.cursor_grid;
                        if cursor_grid == grid && row as f64 == coord.row {
                            if let Some(cell) = vgrid
                                .textbuf()
                                .borrow()
                                .cell(coord.row.floor() as usize, coord.col.floor() as usize)
                            {
                                // self.cursor
                                //     .model_mut()
                                //     .map(|mut m| m.set_cell(cell))
                                //     .unwrap();
                                // self.cursor.update_view().unwrap();
                                trace!("set cursor cell.");
                            } else {
                                error!(
                                    "cursor pos {}x{} of grid {} dose not exists.",
                                    coord.col, row, grid
                                );
                            }
                        }
                    }
                    RedrawEvent::Scroll {
                        grid,
                        top: _,
                        bottom: _,
                        left: _,
                        right: _,
                        rows,
                        columns,
                    } => {
                        let vgrid = self.vgrids.get_mut(grid).unwrap();
                        if rows.is_positive() {
                            vgrid.up(rows.abs() as _);
                        } else if rows.is_negative() {
                            vgrid.down(rows.abs() as _);
                        } else if columns.is_positive() {
                            unimplemented!("scroll left.");
                        } else if columns.is_negative() {
                            unimplemented!("scroll right.");
                        } else {
                            // rows and columns are both zero.
                            unimplemented!("could not be there.");
                        }
                        let cursor_grid = self.cursor_grid;
                        debug!("scrolling grid {} cursor at {}", grid, cursor_grid);
                        if cursor_grid == grid {
                            let coord = &self.cursor_coord;
                            let cell = vgrid
                                .textbuf()
                                .borrow()
                                .cell((coord.row).floor() as usize, (coord.col).floor() as usize)
                                .unwrap();
                            debug!("cursor character change to {}", cell.text);
                            // self.cursor
                            //     .model_mut()
                            //     .map(|mut m| m.set_cell(cell))
                            //     .unwrap();
                            // self.cursor.update_view().unwrap();
                        }
                    }
                    RedrawEvent::Resize {
                        grid,
                        width,
                        height,
                    } => {
                        info!("Resizing grid {} to {}x{}.", grid, width, height);

                        let exists = self.vgrids.get(grid).is_some();
                        if exists {
                            self.vgrids
                                .get_mut(grid)
                                .unwrap()
                                .resize(width as _, height as _);
                        } else {
                            debug!("Add grid {} to default window at left top.", grid);
                            self.vgrids.insert(
                                grid,
                                (
                                    grid,
                                    0,
                                    (0., 0.).into(),
                                    (width, height).into(),
                                    self.hldefs.clone(),
                                    self.dragging.clone(),
                                    self.metrics.clone(),
                                    self.font_description.clone(),
                                ),
                            );
                            self.vgrids
                                .get_mut(grid)
                                .unwrap()
                                .set_pango_context(self.pctx.clone());
                        };
                    }

                    RedrawEvent::WindowPosition {
                        grid,
                        window: _,
                        start_row: row,
                        start_column: column,
                        width,
                        height,
                    } => {
                        // let metrics = self.metrics.get();
                        // let x = start_column as f64 * metrics.width();
                        // let y = start_row as f64 * metrics.height(); //;

                        if self.vgrids.get(grid).is_none() {
                            // dose not exists, create
                            self.vgrids.insert(
                                grid,
                                (
                                    grid,
                                    0,
                                    (column as usize, row as usize).into(),
                                    (width, height).into(),
                                    self.hldefs.clone(),
                                    self.dragging.clone(),
                                    self.metrics.clone(),
                                    self.font_description.clone(),
                                ),
                            );
                            // vgrid.set_pango_context(self.pctx.clone());
                            info!(
                                "Add grid {} at {}x{} with {}x{}.",
                                grid, column, row, height, width
                            );
                        } else {
                            let vgrid = self.vgrids.get_mut(grid).unwrap();
                            vgrid.resize(width as _, height as _);
                            vgrid.set_coord(column as _, row as _);
                            debug!(
                                "Move grid {} to {}x{} with {}x{}.",
                                grid, column, row, height, width
                            );
                            vgrid.show();
                        }

                        info!(
                            "WindowPosition grid {} row-start({}) col-start({}) width({}) height({})",
                            grid, row, column, width, height,
                        );
                    }
                    RedrawEvent::WindowViewport {
                        grid,
                        window: _,
                        top_line,
                        bottom_line,
                        current_line,
                        current_column,
                        line_count,
                    } => {
                        info!(
                            "WindowViewport grid {} viewport: top({}) bottom({}) highlight-line({}) highlight-column({}) with {} lines",
                             grid, top_line, bottom_line, current_line, current_column, line_count,
                        );

                        if self.vgrids.get(grid).is_none() {
                            warn!("WindowViewport before create grid {}.", grid);
                        } else {
                            let vgrid = self.vgrids.get_mut(grid).unwrap();
                            vgrid.show();
                        }
                    }
                    RedrawEvent::WindowHide { grid } => {
                        info!("hide grid {}", grid);
                        self.vgrids.get_mut(grid).unwrap().hide();
                    }
                    RedrawEvent::WindowClose { grid } => {
                        info!("grid {} closed", grid);
                        self.vgrids.remove(grid);
                    }
                    RedrawEvent::Destroy { grid } => {
                        info!("grid {} destroyed", grid);
                        self.vgrids.remove(grid);
                    }
                    RedrawEvent::Flush => {
                        self.vgrids.flush();
                    }
                    RedrawEvent::CursorGoto { grid, row, column } => {
                        let vgrid = self.vgrids.get(grid).unwrap();
                        let leftop = vgrid.coord();
                        let row = row as usize;
                        let column = column as usize;
                        if let Some(cell) = vgrid.textbuf().borrow().cell(row, column) {
                            info!(
                                "cursor goto {}x{} of grid {}, grid at {}x{}",
                                column, row, grid, leftop.col, leftop.row
                            );
                            let coord: Coord =
                                (leftop.col + column as f64, leftop.row + row as f64).into();
                            self.cursor_grid = grid;
                            self.cursor_coord.col = column as _;
                            self.cursor_coord.row = row as _;
                            // self.cursor
                            //     .model_mut()
                            //     .map(|mut m| {
                            //         m.set_cell(cell);
                            //         m.set_grid(grid);
                            //         m.set_coord(coord);
                            //     })
                            //     .unwrap();
                            // self.cursor.update_view().unwrap();
                        } else {
                            warn!(
                                "Cursor pos {}x{} of grid {} dose not exists",
                                row, column, grid
                            );
                        }
                        self.cursor_coord_changed
                            .store(true, atomic::Ordering::Relaxed);
                        self.cursor_grid = grid;
                    }
                    RedrawEvent::ModeInfoSet { cursor_modes } => {
                        self.cursor_modes = cursor_modes;

                        let mode = self.cursor_modes.get(self.cursor_mode).unwrap().clone();
                        // self.cursor
                        //     .model_mut()
                        //     .map(|mut m| {
                        //         m.set_mode(mode);
                        //     })
                        //     .unwrap();
                        // self.cursor.update_view().unwrap();
                    }
                    RedrawEvent::ModeChange { mode, mode_index } => {
                        self.mode = mode;
                        self.cursor_mode = mode_index as _;
                        let cursor_mode = self.cursor_modes.get(self.cursor_mode).unwrap().clone();
                        info!("Mode Change to {:?} {:?}", &self.mode, cursor_mode);
                        // self.cursor
                        //     .model_mut()
                        //     .map(|mut m| {
                        //         m.set_mode(cursor_mode);
                        //     })
                        //     .unwrap();
                        // self.cursor.update_view().unwrap();
                        if matches!(self.mode, EditorMode::Normal | EditorMode::Unknown(_)) {
                            sender.output(AppMessage::ShowPointer).unwrap();
                        }
                    }
                    RedrawEvent::BusyStart => {
                        debug!("Ignored BusyStart.");
                        sender.output(AppMessage::ShowPointer).unwrap();
                    }
                    RedrawEvent::BusyStop => {
                        debug!("Ignored BusyStop.");
                        sender.output(AppMessage::ShowPointer).unwrap();
                    }
                    RedrawEvent::MouseOn => {
                        self.mouse_on.store(true, atomic::Ordering::Relaxed);
                    }
                    RedrawEvent::MouseOff => {
                        self.mouse_on.store(false, atomic::Ordering::Relaxed);
                    }

                    RedrawEvent::MessageShow {
                        kind,
                        content,
                        replace_last,
                    } => {
                        debug!("showing message {:?} {:?}", kind, content);
                        if replace_last && !self.messages.is_empty() {
                            self.messages.guard().pop_back();
                        }

                        self.messages.guard().push_back((
                            kind,
                            content,
                            self.hldefs.clone(),
                            self.metrics.clone(),
                            self.pctx.clone(),
                        ));
                    }
                    RedrawEvent::MessageShowMode { content } => {
                        warn!("message show mode: {:?}", content);
                    }
                    RedrawEvent::MessageRuler { content } => {
                        warn!("message ruler: {:?}", content);
                    }
                    RedrawEvent::MessageSetPosition {
                        grid,
                        row,
                        scrolled,
                        separator_character,
                    } => {
                        debug!(
                            "message set position: {} {} {} '{}'",
                            grid, row, scrolled, separator_character
                        );
                        // let metrics = self.metrics.get();
                        // let y = row as f64 * metrics.height(); //;
                        let width = self.vgrids.get(1).map(|vgrid| vgrid.width()).unwrap();
                        if let Some(vgrid) = self.vgrids.get_mut(grid) {
                            debug!(
                                "moving message grid to 0x{} size {}x{}",
                                row,
                                width,
                                vgrid.height()
                            );
                            vgrid.set_coord(0., row as f64);
                            vgrid.resize(width, vgrid.height());
                            vgrid.show();
                        } else {
                            debug!("creating message grid at 0x{} size {}x{}", row, width, 1);
                            let row = row as usize;
                            self.vgrids.insert(
                                grid,
                                (
                                    grid,
                                    0,
                                    (0, row).into(),
                                    (width, 1).into(),
                                    self.hldefs.clone(),
                                    self.dragging.clone(),
                                    self.metrics.clone(),
                                    self.font_description.clone(),
                                ),
                            );
                            // let mut vgrid = VimGrid::new(
                            //     grid,
                            //     0,
                            //     (0, row).into(),
                            //     (width, 1).into(),
                            //     self.hldefs.clone(),
                            //     self.dragging.clone(),
                            //     self.metrics.clone(),
                            //     self.font_description.clone(),
                            // );
                            // vgrid.set_pango_context(self.pctx.clone());
                        }
                    }
                    RedrawEvent::MessageShowCommand { content } => {
                        warn!("message show command: {:?}", content);
                    }
                    RedrawEvent::MessageHistoryShow { entries } => {
                        warn!("message history: {:?}", entries);
                    }
                    RedrawEvent::MessageClear => {
                        warn!("message clear all");
                        self.messages.guard().clear();
                    }

                    RedrawEvent::WindowFloatPosition {
                        grid,
                        anchor,
                        anchor_grid,
                        anchor_row,
                        anchor_column,
                        focusable,
                        sort_order: _,
                    } => {
                        info!(
                            "grid {} is float window exists in vgrids {} anchor {} {:?} pos {}x{} focusable {}",
                            grid,
                            self.vgrids.get(grid).is_some(),
                            anchor_grid,
                            anchor,
                            anchor_column,
                            anchor_row,
                            focusable
                        );
                        // 避免负值,导致窗口溢出
                        let anchor_column = anchor_column.max(0.);
                        let anchor_row = anchor_row.max(0.);
                        info!("after clamp {}x{}", anchor_column, anchor_row);
                        let coord = self.vgrids.get(anchor_grid).unwrap().coord().clone();
                        // let (left, top) = (basepos.x, basepos.y);

                        let vgrid = self.vgrids.get_mut(grid).unwrap();

                        let (col, row) = match anchor {
                            WindowAnchor::NorthWest => (anchor_column, anchor_row),
                            WindowAnchor::NorthEast => {
                                (anchor_column - vgrid.width() as f64, anchor_row)
                            }
                            WindowAnchor::SouthWest => {
                                (anchor_column, anchor_row - vgrid.height() as f64)
                            }
                            WindowAnchor::SouthEast => (
                                anchor_column - vgrid.width() as f64,
                                anchor_row - vgrid.height() as f64,
                            ),
                        };

                        // let metrics = self.metrics.get();
                        // let x = col * metrics.width();
                        // let y = row * metrics.height();
                        info!("moving float window {} to {}x{}", grid, col, row);
                        vgrid.set_coord(coord.col + col.max(0.), coord.row + row.max(0.));
                        vgrid.set_is_float(true);
                        vgrid.set_focusable(focusable);
                    }

                    RedrawEvent::CommandLineShow {
                        content,
                        position,
                        first_character,
                        prompt,
                        indent,
                        level,
                    } => {
                        self.cmd_prompt
                            .sender()
                            .send(VimCmdEvent::Show(
                                content,
                                position,
                                first_character,
                                prompt,
                                indent,
                                level,
                            ))
                            .unwrap();
                    }
                    RedrawEvent::CommandLineHide => {
                        self.cmd_prompt.sender().send(VimCmdEvent::Hide).unwrap();
                    }
                    RedrawEvent::CommandLineBlockHide => {
                        self.cmd_prompt
                            .sender()
                            .send(VimCmdEvent::BlockHide)
                            .unwrap();
                    }
                    _ => {
                        error!("Unhandled RedrawEvent {:?}", event);
                    }
                }
            }
        }
    }
}
