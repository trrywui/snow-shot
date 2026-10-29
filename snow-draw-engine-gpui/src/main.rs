use gpui::{prelude::*, *};
#[cfg(feature = "native-capture")]
mod native_text_validation;
#[cfg(feature = "native-capture")]
mod native_validation;
use snow_draw_engine_gpui::{
    CanvasDocument, CanvasEvent, CanvasSession, CanvasView, GpuExportFormat, GpuExportJob,
    GpuSavePreparation, SourceImage, engine as draw, showcase_document, write_atomic,
};
#[cfg(feature = "native-capture")]
use std::time::{Duration, Instant};
use std::{collections::HashMap, path::PathBuf, rc::Rc};

const TOOLS: &[(&str, &str, draw::ActiveTool)] = &[
    ("select", "Select  V", draw::ActiveTool::Select),
    ("shape", "Shape  R", draw::ActiveTool::Shape),
    ("arrow", "Arrow  A", draw::ActiveTool::Arrow),
    ("line", "Line  L", draw::ActiveTool::Line),
    ("pen", "Pen  P", draw::ActiveTool::FreeDraw),
    ("text", "Text  T", draw::ActiveTool::Text),
    (
        "rect-highlight",
        "Highlight",
        draw::ActiveTool::RectangleHighlight,
    ),
    ("pen-highlight", "Marker", draw::ActiveTool::PenHighlight),
    (
        "rect-filter",
        "Area filter",
        draw::ActiveTool::RectangleFilter,
    ),
    ("pen-filter", "Brush filter", draw::ActiveTool::PenFilter),
    ("eraser", "Object eraser", draw::ActiveTool::Eraser),
    (
        "rect-eraser",
        "Restore area",
        draw::ActiveTool::RectangleEraser,
    ),
    (
        "brush-eraser",
        "Restore brush",
        draw::ActiveTool::BrushEraser,
    ),
    ("serial", "Number", draw::ActiveTool::SerialNumber),
    ("distance", "Distance", draw::ActiveTool::Distance),
    ("angle", "Angle", draw::ActiveTool::Angle),
    ("bubble", "Speech bubble", draw::ActiveTool::SpeechBubble),
    ("magnifier", "Magnifier", draw::ActiveTool::Magnifier),
    ("spotlight", "Spotlight", draw::ActiveTool::Spotlight),
    ("watermark", "Watermark", draw::ActiveTool::Watermark),
    ("auto-filter", "Auto regions", draw::ActiveTool::AutoFilter),
];

#[derive(Clone, Copy)]
struct Theme {
    background: u32,
    surface: u32,
    text: u32,
    muted: u32,
    border: u32,
    accent: u32,
    selected: u32,
    hover: u32,
}
const LIGHT: Theme = Theme {
    background: 0xf1f3f6,
    surface: 0xffffff,
    text: 0x202b3d,
    muted: 0x748093,
    border: 0xdfe4eb,
    accent: 0x2f64a8,
    selected: 0xe9f1fc,
    hover: 0xf4f6f9,
};
const DARK: Theme = Theme {
    background: 0x171c24,
    surface: 0x222a35,
    text: 0xe4eaf2,
    muted: 0x9caabc,
    border: 0x354252,
    accent: 0x8bb7ef,
    selected: 0x2c405b,
    hover: 0x2c3542,
};

struct Demo {
    canvas: Entity<CanvasView>,
    focus: HashMap<&'static str, FocusHandle>,
    status: SharedString,
    dark: bool,
    custom_crosshair: bool,
    _subscription: Subscription,
    io_task: Option<Task<()>>,
}

impl Demo {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let canvas = cx.new(|cx| {
            CanvasView::new(showcase_document().expect("valid showcase"), window, cx)
                .expect("create canvas")
        });
        let subscription = cx.subscribe(&canvas, |this, _, event, cx| {
            if let CanvasEvent::Error(error) = event {
                this.status = error.clone();
            }
            cx.notify();
        });
        let ids = TOOLS.iter().map(|(id, _, _)| *id).chain([
            "sample",
            "new",
            "open",
            "save",
            "image",
            "png",
            "svg",
            "undo",
            "redo",
            "fit",
            "zoom-in",
            "zoom-out",
            "theme",
            "shape-style",
            "fill-style",
            "stroke-style",
            "width",
            "color",
            "filter-style",
            "back",
            "front",
            "duplicate",
            "align",
            "watermark-text",
            "dirty-regions",
            "clear-cache",
            "custom-crosshair",
        ]);
        let focus = ids
            .map(|id| (id, cx.focus_handle().tab_stop(true)))
            .collect();
        Self {
            canvas,
            focus,
            status: "Ready".into(),
            dark: false,
            custom_crosshair: false,
            _subscription: subscription,
            io_task: None,
        }
    }

    fn button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        selected: bool,
        enabled: bool,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = if self.dark { DARK } else { LIGHT };
        let label = label.into();
        let focus = self.focus[id].clone();
        let click = Rc::new(action);
        let key = click.clone();
        let focus_click = focus.clone();
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label.clone())
            .aria_selected(selected)
            .track_focus(&focus)
            .flex()
            .items_center()
            .justify_center()
            .h_8()
            .px_2()
            .min_w_0()
            .rounded_md()
            .border_1()
            .border_color(rgb(if selected { theme.accent } else { theme.border }))
            .bg(rgb(if selected {
                theme.selected
            } else {
                theme.surface
            }))
            .text_sm()
            .text_color(rgb(if enabled { theme.text } else { theme.muted }))
            .cursor_default()
            .when(enabled, |element| {
                element.hover(move |style| style.bg(rgb(theme.hover)))
            })
            .focus_visible(move |style| style.border_color(rgb(theme.accent)).border_2())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, window, cx| {
                    if enabled {
                        focus_click.focus(window, cx);
                    }
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if enabled {
                    click(this, window, cx);
                }
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if enabled && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    key(this, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(label)
    }

    fn tool(&mut self, tool: draw::ActiveTool, window: &mut Window, cx: &mut Context<Self>) {
        if tool == draw::ActiveTool::Watermark {
            self.canvas.update(cx, |canvas, cx| {
                canvas.command(
                    |engine, viewport| {
                        let mut config = engine.watermark_config().clone();
                        config.text = if config.text.is_empty() {
                            "SNOW DRAW".into()
                        } else {
                            String::new()
                        };
                        engine.set_viewport_watermark_config(viewport, config)
                    },
                    window,
                    cx,
                )
            });
            self.status = "Watermark toggled; undo restores the previous setting".into();
            return;
        }
        if tool == draw::ActiveTool::AutoFilter
            && self
                .canvas
                .read(cx)
                .document()
                .engine()
                .auto_filter_regions()
                .is_none()
        {
            self.canvas.update(cx, |canvas, cx| {
                canvas.command(
                    |engine, viewport| {
                        engine.set_auto_filter_regions(
                            viewport,
                            Some(snow_draw_engine_document::AutoFilterRegionRecord {
                                source_bounds: draw::DrawRect {
                                    min_x: 0.0,
                                    min_y: 0.0,
                                    max_x: 960.0,
                                    max_y: 640.0,
                                },
                                regions: (0..3)
                                    .map(|i| snow_draw_engine_document::AutoFilterRegion {
                                        id: i + 1,
                                        category: "Sample regions".into(),
                                        bounds: draw::DrawRect {
                                            min_x: 70.0 + i as f64 * 230.0,
                                            min_y: 80.0,
                                            max_x: 270.0 + i as f64 * 230.0,
                                            max_y: 125.0,
                                        },
                                    })
                                    .collect(),
                            }),
                        )
                    },
                    window,
                    cx,
                )
            });
            self.status =
                "Sample regions supplied by the demo host; click a region to filter it".into();
        } else {
            self.status = "Ready".into();
        }
        self.canvas
            .update(cx, |canvas, cx| canvas.set_tool(tool, window, cx));
    }
    fn reset(&mut self, sample: bool, window: &mut Window, cx: &mut Context<Self>) {
        let document = if sample {
            showcase_document().unwrap()
        } else {
            CanvasDocument::default()
        };
        let result = self.canvas.update(cx, |canvas, cx| {
            canvas.replace_document(document, window, cx)
        });
        if let Err(error) = result {
            self.status = error.to_string().into();
        } else {
            self.status = if sample {
                "Showcase loaded"
            } else {
                "New drawing"
            }
            .into();
        }
        cx.notify();
    }

    fn open(&mut self, image: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.io_task.is_some() {
            return;
        }
        let dialog = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                if image {
                    "Choose a background image"
                } else {
                    "Open a drawing"
                }
                .into(),
            ),
        });
        self.io_task = Some(cx.spawn_in(window, async move |this, cx| {
            let selection = dialog.await;
            let path = match selection {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => None,
                other => {
                    let _ = this.update(cx, |this, cx| {
                        this.status = format!("Could not open file picker: {other:?}").into();
                        cx.notify();
                    });
                    None
                }
            };
            if let Some(path) = path {
                let loaded = cx
                    .background_executor()
                    .spawn(async move {
                        let bytes = snow_draw_engine_gpui::read_bounded(&path)
                            .map_err(|error| error.to_string())?;
                        if image {
                            SourceImage::decode(&bytes)
                                .map(Loaded::Image)
                                .map_err(|error| error.to_string())
                        } else {
                            snow_draw_engine_gpui::DrawingSnapshot::decode(&bytes)
                                .map(Loaded::Document)
                                .map_err(|error| error.to_string())
                        }
                    })
                    .await;
                let _ = this.update_in(cx, |this, window, cx| {
                    match loaded {
                        Ok(Loaded::Image(image)) => {
                            this.canvas
                                .update(cx, |canvas, cx| canvas.set_background(image, window, cx));
                            this.status = "Background loaded".into();
                        }
                        Ok(Loaded::Document(document)) => {
                            let result = this.canvas.update(cx, |canvas, cx| {
                                document.into_document().and_then(|document| {
                                    canvas.replace_document(document, window, cx)
                                })
                            });
                            this.status = match result {
                                Ok(()) => "Drawing opened".into(),
                                Err(error) => error.to_string().into(),
                            };
                        }
                        Err(error) => this.status = error.into(),
                    }
                    cx.notify();
                });
            }
            let _ = this.update(cx, |this, cx| {
                this.io_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn save(&mut self, format: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.io_task.is_some() {
            return;
        }
        self.canvas.update(cx, |canvas, cx| {
            canvas.set_tool(
                canvas
                    .document()
                    .engine()
                    .viewport_active_tool(canvas.session().viewport())
                    .unwrap_or(draw::ActiveTool::Select),
                window,
                cx,
            )
        });
        let suggested = match format {
            "png" => "drawing.png",
            "svg" => "drawing.svg",
            _ => "drawing.snowdraw",
        };
        let directory = std::env::current_dir().unwrap_or_default();
        let dialog = cx.prompt_for_new_path(&directory, Some(suggested));
        let drawing =
            (format == "drawing").then(|| self.canvas.read(cx).document().persistence_snapshot());
        let snapshot = self.canvas.read(cx).snapshot();
        let smart = self.canvas.read(cx).smart_erase_snapshot();
        let service = cx
            .gpu_service()
            .map_err(|error| snow_draw_engine_gpui::CanvasError::Gpu(error.to_string()));
        self.io_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = match dialog.await {
                Ok(Ok(Some(path))) => Some(
                    async {
                        let service = service?;
                        if format == "drawing" {
                            let mut preparation = GpuSavePreparation::new(
                                service,
                                drawing.expect("drawing format")?,
                                Default::default(),
                            )?;
                            let prepared = loop {
                                if let std::task::Poll::Ready(result) = preparation.poll() {
                                    break result?;
                                }
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(2))
                                    .await;
                            };
                            cx.background_executor()
                                .spawn(async move {
                                    prepared
                                        .encode()
                                        .and_then(|bytes| write_atomic(&path, &bytes))
                                })
                                .await
                        } else {
                            let format = if format == "svg" {
                                GpuExportFormat::Svg
                            } else {
                                GpuExportFormat::Png
                            };
                            let mut job = GpuExportJob::to_path(
                                service,
                                snapshot?,
                                2.0,
                                smart,
                                format,
                                path,
                                Default::default(),
                            )?;
                            loop {
                                if let std::task::Poll::Ready(result) = job.poll() {
                                    break result;
                                }
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(2))
                                    .await;
                            }
                        }
                    }
                    .await,
                ),
                Ok(Ok(None)) => None,
                other => Some(Err(snow_draw_engine_gpui::CanvasError::InvalidFile(
                    format!("Could not open save dialog: {other:?}"),
                ))),
            };
            let _ = this.update(cx, |this, cx| {
                this.io_task = None;
                if let Some(result) = result {
                    this.status = match result {
                        Ok(()) => "Saved".into(),
                        Err(error) => error.to_string().into(),
                    };
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn mutate_style(&mut self, property: &str, window: &mut Window, cx: &mut Context<Self>) {
        let state = self
            .canvas
            .read(cx)
            .document()
            .engine()
            .viewport_style_toolbar_state(self.canvas.read(cx).session().viewport())
            .unwrap();
        let mut style = state.shape_style;
        let (_, mask) = match property {
            "shape" => {
                style.shape = match style.shape {
                    draw::HighlightShape::Rectangle => draw::HighlightShape::Ellipse,
                    draw::HighlightShape::Ellipse => draw::HighlightShape::Diamond,
                    _ => draw::HighlightShape::Rectangle,
                };
                (
                    draw::ShapeKind::Rectangle,
                    snow_draw_engine_editor::SHAPE_STYLE_PROPERTY_SHAPE,
                )
            }
            "fill" => {
                style.fill_style = match style.fill_style {
                    draw::FillStyle::Border => draw::FillStyle::Solid,
                    draw::FillStyle::Solid => draw::FillStyle::Line,
                    draw::FillStyle::Line => draw::FillStyle::CrossLine,
                    _ => draw::FillStyle::Border,
                };
                style.fill = draw::ColorRgba8 {
                    r: 211,
                    g: 229,
                    b: 251,
                    a: 255,
                };
                (
                    draw::ShapeKind::Rectangle,
                    snow_draw_engine_editor::SHAPE_STYLE_PROPERTY_FILL_STYLE
                        | snow_draw_engine_editor::SHAPE_STYLE_PROPERTY_FILL,
                )
            }
            "stroke" => {
                style.stroke_style = match style.stroke_style {
                    draw::StrokeStyle::Solid => draw::StrokeStyle::Dashed,
                    draw::StrokeStyle::Dashed => draw::StrokeStyle::Dotted,
                    _ => draw::StrokeStyle::Solid,
                };
                (
                    draw::ShapeKind::Rectangle,
                    snow_draw_engine_editor::SHAPE_STYLE_PROPERTY_STROKE_STYLE,
                )
            }
            "width" => {
                style.stroke_width = if style.stroke_width >= 8.0 {
                    1.0
                } else {
                    style.stroke_width + 1.0
                };
                (
                    draw::ShapeKind::Rectangle,
                    snow_draw_engine_editor::SHAPE_STYLE_PROPERTY_STROKE_WIDTH,
                )
            }
            _ => {
                style.stroke = if style.stroke.r > 150 {
                    draw::ColorRgba8 {
                        r: 47,
                        g: 100,
                        b: 168,
                        a: 255,
                    }
                } else {
                    draw::ColorRgba8 {
                        r: 187,
                        g: 73,
                        b: 83,
                        a: 255,
                    }
                };
                (
                    draw::ShapeKind::Rectangle,
                    snow_draw_engine_editor::SHAPE_STYLE_PROPERTY_STROKE,
                )
            }
        };
        let kind = match self
            .canvas
            .read(cx)
            .document()
            .engine()
            .viewport_active_tool(self.canvas.read(cx).session().viewport())
            .unwrap_or(draw::ActiveTool::Shape)
        {
            draw::ActiveTool::Arrow => draw::ShapeKind::Arrow,
            draw::ActiveTool::Line => draw::ShapeKind::Line,
            draw::ActiveTool::FreeDraw => draw::ShapeKind::FreeDraw,
            draw::ActiveTool::RectangleHighlight => draw::ShapeKind::RectangleHighlight,
            draw::ActiveTool::PenHighlight => draw::ShapeKind::PenHighlight,
            draw::ActiveTool::Spotlight => draw::ShapeKind::Spotlight,
            draw::ActiveTool::SpeechBubble => draw::ShapeKind::SpeechBubble,
            _ => draw::ShapeKind::Rectangle,
        };
        self.canvas.update(cx, |canvas, cx| {
            canvas.command(
                |engine, viewport| {
                    engine.set_viewport_shape_style_patch(
                        viewport,
                        draw::ShapeStylePatch {
                            kind,
                            style,
                            properties: mask,
                        },
                    )
                },
                window,
                cx,
            )
        });
    }
}

enum Loaded {
    Image(SourceImage),
    Document(snow_draw_engine_gpui::DrawingSnapshot),
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = if self.dark { DARK } else { LIGHT };
        let active = self
            .canvas
            .read(cx)
            .document()
            .engine()
            .viewport_active_tool(self.canvas.read(cx).session().viewport())
            .unwrap_or(draw::ActiveTool::Select);
        let history = self.canvas.read(cx).document().engine().history_state();
        let zoom = self.canvas.read(cx).session().scene().frame().camera.zoom;
        let selected = self
            .canvas
            .read(cx)
            .document()
            .engine()
            .selected_ids()
            .len();
        let filter = self
            .canvas
            .read(cx)
            .document()
            .engine()
            .viewport_style_toolbar_state(self.canvas.read(cx).session().viewport())
            .unwrap()
            .filter_style;
        let busy = self.io_task.is_some();
        let mut tools = div().flex().flex_col().gap_2();
        for row in TOOLS.chunks(2) {
            let mut element = div().flex().gap_2();
            for &(id, label, tool) in row {
                element = element.child(div().flex_1().min_w_0().child(self.button(
                    id,
                    label,
                    active == tool,
                    true,
                    move |this, window, cx| this.tool(tool, window, cx),
                    cx,
                )));
            }
            tools = tools.child(element);
        }
        div().size_full().flex().flex_col().bg(rgb(theme.background)).text_color(rgb(theme.text)).font_family("Segoe UI").text_sm()
            .child(div().flex().items_center().gap_2().px_4().py_3().bg(rgb(theme.surface)).border_b_1().border_color(rgb(theme.border))
                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).mr_3().child("Snow Draw"))
                .child(self.button("sample","Showcase",false,!busy,|this,w,cx|this.reset(true,w,cx),cx))
                .child(self.button("new","New",false,!busy,|this,w,cx|this.reset(false,w,cx),cx))
                .child(self.button("open","Open…",false,!busy,|this,w,cx|this.open(false,w,cx),cx))
                .child(self.button("save","Save…",false,!busy,|this,w,cx|this.save("drawing",w,cx),cx))
                .child(self.button("image","Image…",false,!busy,|this,w,cx|this.open(true,w,cx),cx))
                .child(div().flex_1())
                .child(self.button("png","PNG…",false,!busy,|this,w,cx|this.save("png",w,cx),cx))
                .child(self.button("svg","SVG…",false,!busy,|this,w,cx|this.save("svg",w,cx),cx))
                .child(self.button("theme",if self.dark{"Light"}else{"Dark"},false,true,|this,_,cx|{this.dark = !this.dark;cx.notify();},cx)))
            .child(div().flex().items_stretch().flex_1().min_h_0()
                .child(div().id("tools-panel").w_64().flex_shrink_0().overflow_y_scroll().flex().flex_col().gap_4().p_4().bg(rgb(theme.surface)).border_r_1().border_color(rgb(theme.border))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Drawing tools"))
                    .child(tools)
                    .child(div().border_t_1().border_color(rgb(theme.border)).pt_3().flex().flex_col().gap_2()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child("Appearance"))
                        .child(self.button("shape-style","Cycle shape",false,true,|this,w,cx|this.mutate_style("shape",w,cx),cx))
                        .child(self.button("fill-style","Cycle fill",false,true,|this,w,cx|this.mutate_style("fill",w,cx),cx))
                        .child(div().flex().gap_2().child(self.button("stroke-style","Dash",false,true,|this,w,cx|this.mutate_style("stroke",w,cx),cx)).child(self.button("width","Width +",false,true,|this,w,cx|this.mutate_style("width",w,cx),cx)).child(self.button("color","Color",false,true,|this,w,cx|this.mutate_style("color",w,cx),cx)))
                        .child(self.button("filter-style",format!("Filter: {:?}",filter.filter_type),false,true,|this,w,cx|{
                            let mut filter=this.canvas.read(cx).document().engine().viewport_style_toolbar_state(this.canvas.read(cx).session().viewport()).unwrap().filter_style;
                            filter.filter_type=match filter.filter_type{draw::CanvasFilterType::Mosaic=>draw::CanvasFilterType::GaussianBlur,draw::CanvasFilterType::GaussianBlur=>draw::CanvasFilterType::Grayscale,draw::CanvasFilterType::Grayscale=>draw::CanvasFilterType::Inversion,draw::CanvasFilterType::Inversion=>draw::CanvasFilterType::Emboss,draw::CanvasFilterType::Emboss=>draw::CanvasFilterType::Brightness,draw::CanvasFilterType::Brightness=>draw::CanvasFilterType::SmartErase,_=>draw::CanvasFilterType::Mosaic};
                            this.canvas.update(cx,|canvas,cx|canvas.command(|engine,viewport|engine.set_viewport_filter_style(viewport,filter,draw::FILTER_STYLE_PROPERTY_ALL),w,cx));
                        },cx)))
                    .child(div().text_xs().text_color(rgb(theme.muted)).child("Space + drag to pan\nWheel to zoom or adjust tool\nShift to constrain or add selection\nDouble-click text to edit\nCtrl+Enter to finish; Esc to cancel")))
                .child(div().flex().flex_col().flex_1().min_w_0().min_h_0()
                    .child(div().flex().items_center().gap_2().px_4().py_2().bg(rgb(theme.surface)).border_b_1().border_color(rgb(theme.border))
                        .child(self.button("undo","Undo",false,history.can_undo,|this,w,cx|this.canvas.update(cx,|canvas,cx|canvas.command(|engine,_|engine.undo_with_viewport_changes(),w,cx)),cx))
                        .child(self.button("redo","Redo",false,history.can_redo,|this,w,cx|this.canvas.update(cx,|canvas,cx|canvas.command(|engine,_|engine.redo_with_viewport_changes(),w,cx)),cx))
                        .child(self.button("duplicate","Duplicate",false,selected>0,|this,w,cx|this.canvas.update(cx,|canvas,cx|canvas.command(|engine,viewport|engine.duplicate_selected_with_viewport_changes(viewport,draw::Point::new(20.0,20.0)),w,cx)),cx))
                        .child(self.button("back","To back",false,selected>0,|this,w,cx|this.canvas.update(cx,|canvas,cx|canvas.command(|engine,viewport|engine.reorder_selected_with_viewport_changes(viewport,0),w,cx)),cx))
                        .child(self.button("front","To front",false,selected>0,|this,w,cx|this.canvas.update(cx,|canvas,cx|canvas.command(|engine,viewport|engine.reorder_selected_with_viewport_changes(viewport,3),w,cx)),cx))
                        .child(div().flex_1())
                        .child(self.button("zoom-out","−",false,true,|this,w,cx|this.canvas.update(cx,|canvas,cx|canvas.zoom(1.0/1.2,w,cx)),cx))
                        .child(div().w_12().text_center().child(format!("{:.0}%",zoom*100.0)))
                        .child(self.button("zoom-in","+",false,true,|this,w,cx|this.canvas.update(cx,|canvas,cx|canvas.zoom(1.2,w,cx)),cx))
                        .child(self.button("fit","Fit",false,true,|this,w,cx|this.canvas.update(cx,|canvas,cx|canvas.fit(w,cx)),cx)))
                    .child(div().flex_1().min_h_0().min_w_0().m_4().border_1().border_color(rgb(theme.border)).child(self.canvas.clone()))))
            .child(div().flex().items_center().gap_3().px_4().py_2().bg(rgb(theme.surface)).border_t_1().border_color(rgb(theme.border)).text_xs().text_color(rgb(theme.muted))
                .child(if busy{"Working…".into()}else{self.status.clone()})
                .child(div().flex_1())
                .child(self.button("dirty-regions","Damage",self.canvas.read(cx).show_dirty_regions(),true,|this,_,cx|{
                    this.canvas.update(cx,|view,cx|view.set_show_dirty_regions(!view.show_dirty_regions(),cx));
                },cx))
                .child(self.button("clear-cache","Clear cache",false,true,|this,w,cx|this.canvas.update(cx,|view,cx|view.clear_render_state(w,cx)),cx))
                .child(self.button("custom-crosshair","Crosshair",self.custom_crosshair,true,|this,_,cx|{
                    this.custom_crosshair = !this.custom_crosshair;
                    let factory=if this.custom_crosshair {Some(Rc::new(|scale:f32|Some(snow_draw_engine_gpui::CanvasCrosshair{
                        radius:18.0/scale,gap:5.0/scale,width:(1.0/scale).max(0.25),color:rgb(0xd65337).into(),
                    })) as snow_draw_engine_gpui::CanvasCrosshairFactory)}else{None};
                    this.canvas.update(cx,|view,cx|view.set_crosshair_factory(factory,cx));
                    cx.notify();
                },cx))
                .child(format!("{selected} selected"))
                .child("GPUI · Apache-2.0"))
    }
}

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|argument| argument == "--export-sample")
    {
        let destination = PathBuf::from(arguments.get(1).expect("--export-sample <output.png>"));
        let mut document = showcase_document().expect("sample document");
        let mut session = CanvasSession::new(&mut document).unwrap();
        session.set_surface(&mut document, 960, 640).unwrap();
        session
            .set_camera(
                &mut document,
                draw::Camera {
                    center: draw::Point::new(480.0, 320.0),
                    zoom: 1.0,
                },
            )
            .unwrap();
        snow_draw_engine_gpui::prepare_text_metrics(&mut document, &mut session).unwrap();
        let snapshot = snow_draw_engine_gpui::CanvasSnapshot::capture(&document, &session).unwrap();
        let source = document.background().cloned();
        let filters: Vec<_> = session
            .scene()
            .nodes()
            .filter_map(|(_, node)| {
                if let draw::SceneDisplayItem::Filter(filter) = node.item.as_ref()
                    && filter.filter.filter_type == draw::DisplayFilterType::SmartErase
                {
                    Some(filter.clone())
                } else {
                    None
                }
            })
            .collect();
        let smart = std::thread::spawn(move || {
            let mut service = snow_draw_engine_gpui::SmartEraseService::default();
            if let Some(source) = source {
                for filter in filters {
                    service
                        .solve(&filter, &source, &Default::default())
                        .expect("sample Smart Erase");
                }
            }
            service.snapshot()
        })
        .join()
        .expect("Smart Erase worker");
        let host =
            gpui_windows::gpu_extensions::OffscreenGpuHost::new().expect("GPUI offscreen host");
        let mut job = GpuExportJob::to_path(
            host.service(),
            snapshot,
            1.0,
            smart,
            GpuExportFormat::Png,
            destination,
            Default::default(),
        )
        .expect("sample GPU export");
        loop {
            if let std::task::Poll::Ready(result) = job.poll() {
                result.expect("write sample export");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        return;
    }
    let text_capture = arguments
        .first()
        .is_some_and(|argument| argument == "--capture-text");
    let capture = if text_capture
        || arguments
            .first()
            .is_some_and(|argument| argument == "--capture")
    {
        Some(PathBuf::from(
            arguments
                .get(1)
                .expect("--capture[-text] <output directory>"),
        ))
    } else {
        None
    };
    let capture_failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let exit_status = capture_failed.clone();
    gpui_platform::application().run(move |cx| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let (window, demo) = {
            let bounds = Bounds::centered(None, size(px(1280.0), px(880.0)), cx);
            let mut entity = None;
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Snow Draw Engine — GPUI".into()),
                            ..Default::default()
                        }),
                        window_min_size: Some(size(px(1024.0), px(720.0))),
                        ..Default::default()
                    },
                    |window, cx| {
                        let demo = cx.new(|cx| Demo::new(window, cx));
                        entity = Some(demo.clone());
                        demo
                    },
                )
                .expect("open demo window");
            (window, entity.unwrap())
        };
        cx.activate(true);
        if let Some(directory) = capture {
            #[cfg(feature = "native-capture")]
            cx.spawn(async move |cx| {
                let result = if text_capture {
                    native_text_validation::run(window.into(), demo, directory, cx).await
                } else {
                    native_validation::run(window.into(), demo, directory, cx).await
                };
                if let Err(error) = result {
                    capture_failed.store(true, std::sync::atomic::Ordering::Relaxed);
                    eprintln!("Native validation failed: {error:#}");
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
            #[cfg(not(feature = "native-capture"))]
            {
                let _ = (directory, window, demo, text_capture);
                eprintln!("Native capture requires --features native-capture");
                capture_failed.store(true, std::sync::atomic::Ordering::Relaxed);
                cx.quit();
            }
        }
    });
    if exit_status.load(std::sync::atomic::Ordering::Relaxed) {
        std::process::exit(1);
    }
}
