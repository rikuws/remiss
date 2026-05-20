use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    mem,
    ops::Range,
    rc::Rc,
    time::Duration,
};

use gpui::{
    fill, point, px, size, AnyTooltip, AnyView, App, Bounds, ClipboardItem, DispatchPhase, Element,
    ElementId, FocusHandle, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId,
    IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, SharedString, StyledText, Task, TextLayout, TextRun, Window,
    WrappedLineLayout,
};

use crate::{
    shortcuts,
    state::AppState,
    theme::{accent, accent_muted},
};

thread_local! {
    static ACTIVE_TEXT_TARGET: RefCell<Option<String>> = const { RefCell::new(None) };
    static TEXT_SELECTION_GROUPS: RefCell<HashMap<String, GroupTextSelectionState>> = RefCell::new(HashMap::new());
}

const TEXT_TOOLTIP_SHOW_DELAY: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppTextFieldKind {
    PaletteQuery,
    ReviewBody,
    WaymarkDraft,
    InlineCommentDraft,
    WaypointSpotlightQuery,
}

#[derive(Default)]
struct TextSelectionState {
    anchor_index: Option<usize>,
    head_index: Option<usize>,
    mouse_down_index: Option<usize>,
    selecting: bool,
    hovered_index: Option<usize>,
}

#[derive(Clone)]
struct VisibleTextTooltip {
    key: String,
    mouse_position: gpui::Point<Pixels>,
    view: AnyView,
}

struct PendingTextTooltip {
    key: String,
    _show_task: Task<()>,
}

#[derive(Default)]
struct TextTooltipState {
    active: Option<VisibleTextTooltip>,
    pending: Option<PendingTextTooltip>,
}

impl TextTooltipState {
    fn clear(&mut self) {
        self.active = None;
        self.pending = None;
    }

    fn has_key(&self, key: &str) -> bool {
        self.active
            .as_ref()
            .map(|tooltip| tooltip.key.as_str() == key)
            .unwrap_or(false)
            || self
                .pending
                .as_ref()
                .map(|tooltip| tooltip.key.as_str() == key)
                .unwrap_or(false)
    }
}

impl TextSelectionState {
    fn clamp(&mut self, len: usize) {
        self.anchor_index = self.anchor_index.map(|index| index.min(len));
        self.head_index = self.head_index.map(|index| index.min(len));
        self.mouse_down_index = self.mouse_down_index.map(|index| index.min(len));
    }

    fn selection_range(&self) -> Option<Range<usize>> {
        let anchor = self.anchor_index?;
        let head = self.head_index.unwrap_or(anchor);
        Some(anchor.min(head)..anchor.max(head))
    }

    fn cursor_index(&self) -> usize {
        self.head_index.or(self.anchor_index).unwrap_or(0)
    }

    fn collapse_to(&mut self, index: usize) {
        self.anchor_index = Some(index);
        self.head_index = Some(index);
    }

    fn select_to(&mut self, index: usize) {
        if self.anchor_index.is_none() {
            self.anchor_index = Some(index);
        }
        self.head_index = Some(index);
    }

    fn select_all(&mut self, len: usize) {
        self.anchor_index = Some(0);
        self.head_index = Some(len);
    }

    fn clear(&mut self) {
        self.anchor_index = None;
        self.head_index = None;
        self.mouse_down_index = None;
        self.selecting = false;
    }
}

struct SelectableTextClickEvent {
    mouse_down_index: usize,
    mouse_up_index: usize,
}

#[derive(Clone)]
struct GroupTextSelectionConfig {
    group_id: String,
    row_order: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct GroupTextPoint {
    row_order: i64,
    index: usize,
}

#[derive(Default)]
struct GroupTextSelectionState {
    rows: BTreeMap<i64, String>,
    anchor: Option<GroupTextPoint>,
    head: Option<GroupTextPoint>,
    mouse_down: Option<GroupTextPoint>,
    selecting: bool,
}

#[doc(hidden)]
#[derive(Default)]
pub struct SelectableTextState {
    focus_handle: Option<FocusHandle>,
    selection: Rc<RefCell<TextSelectionState>>,
    tooltip: Rc<RefCell<TextTooltipState>>,
}

pub struct SelectableText {
    element_id: ElementId,
    selection_id: String,
    raw_text: SharedString,
    text: StyledText,
    click_listener: Option<
        Box<dyn Fn(&[Range<usize>], SelectableTextClickEvent, &mut Window, &mut App) -> bool>,
    >,
    click_requires_platform_modifier: bool,
    unmatched_click_listener: Option<Box<dyn Fn(&mut Window, &mut App)>>,
    hover_listener: Option<Box<dyn Fn(Option<usize>, MouseMoveEvent, &mut Window, &mut App)>>,
    tooltip_builder: Option<Rc<dyn Fn(usize, &mut Window, &mut App) -> Option<(String, AnyView)>>>,
    clickable_ranges: Vec<Range<usize>>,
    selection_color: gpui::Rgba,
    selection_group: Option<GroupTextSelectionConfig>,
}

impl SelectableText {
    pub fn new(id: impl Into<SharedString>, text: impl Into<SharedString>) -> Self {
        let raw_text = text.into();
        let selection_id: SharedString = id.into();
        let element_id = ElementId::Name(selection_id.clone());

        Self {
            element_id,
            selection_id: selection_id.to_string(),
            text: StyledText::new(raw_text.clone()),
            raw_text,
            click_listener: None,
            click_requires_platform_modifier: false,
            unmatched_click_listener: None,
            hover_listener: None,
            tooltip_builder: None,
            clickable_ranges: Vec::new(),
            selection_color: accent_muted(),
            selection_group: None,
        }
    }

    pub fn with_runs(mut self, runs: Vec<TextRun>) -> Self {
        self.text = StyledText::new(self.raw_text.clone()).with_runs(runs);
        self
    }

    pub fn on_click(
        mut self,
        ranges: Vec<Range<usize>>,
        listener: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.click_listener = Some(Box::new(move |ranges, event, window, cx| {
            for (range_ix, range) in ranges.iter().enumerate() {
                if range.contains(&event.mouse_down_index) && range.contains(&event.mouse_up_index)
                {
                    listener(range_ix, window, cx);
                    return true;
                }
            }
            false
        }));
        self.clickable_ranges = ranges;
        self
    }

    pub fn require_platform_modifier_for_click(mut self) -> Self {
        self.click_requires_platform_modifier = true;
        self
    }

    pub fn on_click_unmatched(
        mut self,
        listener: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.unmatched_click_listener = Some(Box::new(listener));
        self
    }

    pub fn on_hover(
        mut self,
        listener: impl Fn(Option<usize>, MouseMoveEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.hover_listener = Some(Box::new(listener));
        self
    }

    pub fn tooltip(
        mut self,
        builder: impl Fn(usize, &mut Window, &mut App) -> Option<AnyView> + 'static,
    ) -> Self {
        self.tooltip_builder = Some(Rc::new(move |index, window, cx| {
            builder(index, window, cx).map(|view| (index.to_string(), view))
        }));
        self
    }

    pub fn tooltip_with_key(
        mut self,
        builder: impl Fn(usize, &mut Window, &mut App) -> Option<(String, AnyView)> + 'static,
    ) -> Self {
        self.tooltip_builder = Some(Rc::new(builder));
        self
    }

    pub fn selection_group(mut self, group_id: impl Into<String>, row_order: i64) -> Self {
        self.selection_group = Some(GroupTextSelectionConfig {
            group_id: group_id.into(),
            row_order,
        });
        self
    }
}

impl Element for SelectableText {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.element_id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.text.request_layout(None, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        window.with_optional_element_state::<SelectableTextState, _>(
            global_id,
            |selectable_state, window| {
                let mut selectable_state =
                    selectable_state.map(|selectable_state| selectable_state.unwrap_or_default());
                if let Some(selectable_state) = selectable_state.as_mut() {
                    let focus_handle = selectable_state
                        .focus_handle
                        .get_or_insert_with(|| cx.focus_handle())
                        .clone();
                    window.set_focus_handle(&focus_handle, cx);
                }

                self.text
                    .prepaint(None, inspector_id, bounds, state, window, cx);
                let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);

                if self.tooltip_builder.is_some() {
                    let selection_state = selectable_state
                        .as_ref()
                        .map(|state| state.selection.clone())
                        .unwrap_or_default();
                    let tooltip_state = selectable_state
                        .as_ref()
                        .map(|state| state.tooltip.clone())
                        .unwrap_or_default();
                    if selection_state.borrow().selecting {
                        tooltip_state.borrow_mut().clear();
                    }

                    let active_tooltip = tooltip_state.borrow().active.clone();
                    if let Some(active_tooltip) = active_tooltip {
                        let source_bounds = bounds;
                        let selection_state = selection_state.clone();
                        let tooltip_state = tooltip_state.clone();
                        let tooltip_key = active_tooltip.key.clone();
                        window.set_tooltip(AnyTooltip {
                            view: active_tooltip.view,
                            mouse_position: active_tooltip.mouse_position,
                            check_visible_and_update: Rc::new(
                                move |tooltip_bounds, window, _cx| {
                                    let mouse_position = window.mouse_position();
                                    let visible = !selection_state.borrow().selecting
                                        && (source_bounds.contains(&mouse_position)
                                            || tooltip_bounds.contains(&mouse_position));

                                    if !visible {
                                        let mut state = tooltip_state.borrow_mut();
                                        if state
                                            .active
                                            .as_ref()
                                            .map(|tooltip| tooltip.key == tooltip_key)
                                            .unwrap_or(false)
                                        {
                                            state.active = None;
                                        }
                                        if state
                                            .pending
                                            .as_ref()
                                            .map(|tooltip| tooltip.key == tooltip_key)
                                            .unwrap_or(false)
                                        {
                                            state.pending = None;
                                        }
                                    }

                                    visible
                                },
                            ),
                        });
                    }
                }

                (hitbox, selectable_state)
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let text_layout = self.text.layout().clone();
        let selection_id = self.selection_id.clone();
        let raw_text = self.raw_text.clone();
        let selection_group = self.selection_group.clone();

        window.with_element_state::<SelectableTextState, _>(
            global_id.unwrap(),
            |selectable_state, window| {
                let selectable_state = selectable_state.unwrap_or_default();
                let focus_handle = selectable_state.focus_handle.clone();
                let selection_state = selectable_state.selection.clone();
                selection_state.borrow_mut().clamp(raw_text.len());
                if let Some(group) = selection_group.as_ref() {
                    register_group_text_row(group, raw_text.as_ref());
                }

                if let Some(hover_listener) = self.hover_listener.take() {
                    let hover_selection = selection_state.clone();
                    let hover_hitbox = hitbox.clone();
                    let hover_layout = text_layout.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }

                        let hovered = hover_hitbox.is_hovered(window).then(|| {
                            hover_layout
                                .index_for_position(event.position)
                                .unwrap_or_else(|index| index)
                        });

                        if hover_selection.borrow().hovered_index == hovered {
                            return;
                        }

                        hover_selection.borrow_mut().hovered_index = hovered;
                        hover_listener(hovered, event.clone(), window, cx);
                        window.refresh();
                    });
                }

                if let Some(tooltip_builder) = self.tooltip_builder.clone() {
                    let tooltip_selection = selection_state.clone();
                    let tooltip_state = selectable_state.tooltip.clone();
                    let tooltip_hitbox = hitbox.clone();
                    let tooltip_layout = text_layout.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }

                        if tooltip_selection.borrow().selecting {
                            let had_active = tooltip_state.borrow().active.is_some();
                            tooltip_state.borrow_mut().clear();
                            if had_active {
                                window.refresh();
                            }
                            return;
                        }

                        if !tooltip_hitbox.is_hovered(window) {
                            tooltip_state.borrow_mut().pending = None;
                            return;
                        }

                        let Ok(index) = tooltip_layout.index_for_position(event.position) else {
                            let had_active = tooltip_state.borrow().active.is_some();
                            tooltip_state.borrow_mut().clear();
                            if had_active {
                                window.refresh();
                            }
                            return;
                        };

                        let Some((key, view)) = tooltip_builder(index, window, cx) else {
                            let had_active = tooltip_state.borrow().active.is_some();
                            tooltip_state.borrow_mut().clear();
                            if had_active {
                                window.refresh();
                            }
                            return;
                        };

                        if tooltip_state.borrow().has_key(&key) {
                            return;
                        }

                        let mouse_position = event.position;
                        let show_task = window.spawn(cx, {
                            let tooltip_state = tooltip_state.clone();
                            let key = key.clone();
                            let view = view.clone();
                            async move |cx| {
                                cx.background_executor()
                                    .timer(TEXT_TOOLTIP_SHOW_DELAY)
                                    .await;
                                cx.update(|window, _cx| {
                                    let mut state = tooltip_state.borrow_mut();
                                    let should_show = state
                                        .pending
                                        .as_ref()
                                        .map(|tooltip| tooltip.key == key)
                                        .unwrap_or(false);
                                    if should_show {
                                        state.active = Some(VisibleTextTooltip {
                                            key,
                                            mouse_position,
                                            view,
                                        });
                                        state.pending = None;
                                        window.refresh();
                                    }
                                })
                                .ok();
                            }
                        });

                        let mut state = tooltip_state.borrow_mut();
                        state.active = None;
                        state.pending = Some(PendingTextTooltip {
                            key,
                            _show_task: show_task,
                        });
                    });
                }

                let clear_selection_hitbox = hitbox.clone();
                let clear_selection_id = selection_id.clone();
                let clear_selection_state = selection_state.clone();
                let clear_selection_group = selection_group.clone();
                window.on_mouse_event(move |_event: &MouseDownEvent, phase, window, _cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if !is_active_text_target(&clear_selection_id) {
                        return;
                    }
                    if clear_selection_hitbox.is_hovered(window) {
                        return;
                    }

                    clear_selection_state.borrow_mut().clear();
                    if let Some(group) = clear_selection_group.as_ref() {
                        clear_group_text_selection(group);
                    }
                    clear_active_text_target(&clear_selection_id);
                    window.refresh();
                });

                let mouse_down_hitbox = hitbox.clone();
                let mouse_down_layout = text_layout.clone();
                let mouse_down_selection = selection_state.clone();
                let mouse_down_id = selection_id.clone();
                let mouse_down_group = selection_group.clone();
                let mouse_down_text = raw_text.clone();
                let mouse_down_focus = focus_handle.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture
                        || event.button != MouseButton::Left
                        || !mouse_down_hitbox.is_hovered(window)
                    {
                        return;
                    }

                    let index = mouse_down_layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|index| index);

                    {
                        let mut state = mouse_down_selection.borrow_mut();
                        state.mouse_down_index = Some(index);
                        state.selecting = true;
                        if event.modifiers.shift && state.anchor_index.is_some() {
                            state.select_to(index);
                        } else {
                            state.collapse_to(index);
                        }
                    }
                    if let Some(group) = mouse_down_group.as_ref() {
                        register_group_text_row(group, mouse_down_text.as_ref());
                        start_group_text_selection(group, index, event.modifiers.shift);
                    }

                    if let Some(focus_handle) = mouse_down_focus.as_ref() {
                        focus_handle.focus(window);
                    }
                    set_active_text_target(mouse_down_id.clone());
                    cx.stop_propagation();
                    window.refresh();
                });

                let mouse_move_selection = selection_state.clone();
                let mouse_move_layout = text_layout.clone();
                let mouse_move_hitbox = hitbox.clone();
                let mouse_move_group = selection_group.clone();
                let mouse_move_text = raw_text.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }

                    if let Some(group) = mouse_move_group.as_ref() {
                        register_group_text_row(group, mouse_move_text.as_ref());
                        if is_group_text_selecting(group) && mouse_move_hitbox.is_hovered(window) {
                            let index = mouse_move_layout
                                .index_for_position(event.position)
                                .unwrap_or_else(|index| index);
                            update_group_text_selection(group, index);
                            window.refresh();
                        }
                    }

                    if !mouse_move_selection.borrow().selecting {
                        return;
                    }

                    let index = mouse_move_layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|index| index);
                    mouse_move_selection.borrow_mut().select_to(index);
                    window.refresh();
                });

                let mouse_up_selection = selection_state.clone();
                let mouse_up_layout = text_layout.clone();
                let mouse_up_hitbox = hitbox.clone();
                let mouse_up_group = selection_group.clone();
                let mouse_up_text = raw_text.clone();
                let cursor_click_ranges = self.clickable_ranges.clone();
                let click_requires_platform_modifier = self.click_requires_platform_modifier;
                let click_ranges = mem::take(&mut self.clickable_ranges);
                let click_listener = self.click_listener.take();
                let unmatched_click_listener = self.unmatched_click_listener.take();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                        return;
                    }

                    let maybe_mouse_down = mouse_up_selection.borrow().mouse_down_index;
                    if maybe_mouse_down.is_none() && !mouse_up_selection.borrow().selecting {
                        return;
                    }

                    let mouse_up_index = mouse_up_layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|index| index);
                    if let Some(group) = mouse_up_group.as_ref() {
                        register_group_text_row(group, mouse_up_text.as_ref());
                        if mouse_up_hitbox.is_hovered(window) {
                            update_group_text_selection(group, mouse_up_index);
                        }
                        finish_group_text_selection(group);
                    }

                    let mut state = mouse_up_selection.borrow_mut();
                    if state.selecting {
                        state.select_to(mouse_up_index);
                    }
                    state.selecting = false;
                    let mouse_down_index = state.mouse_down_index.take();
                    let had_mouse_down = mouse_down_index.is_some();
                    let selection_range = state.selection_range();
                    drop(state);

                    if let (Some(mouse_down_index), Some(listener)) =
                        (mouse_down_index, click_listener.as_ref())
                    {
                        let collapsed = selection_range
                            .as_ref()
                            .map(|range| range.is_empty())
                            .unwrap_or(false);
                        if collapsed {
                            let click_allowed = !click_requires_platform_modifier
                                || platform_click_modifier(event.modifiers);
                            let handled_click = click_allowed
                                && listener(
                                    &click_ranges,
                                    SelectableTextClickEvent {
                                        mouse_down_index,
                                        mouse_up_index,
                                    },
                                    window,
                                    cx,
                                );

                            if !handled_click {
                                if let Some(listener) = unmatched_click_listener.as_ref() {
                                    listener(window, cx);
                                }
                            }
                        }
                    } else if had_mouse_down
                        && selection_range
                            .as_ref()
                            .map(|range| range.is_empty())
                            .unwrap_or(false)
                    {
                        if let Some(listener) = unmatched_click_listener.as_ref() {
                            listener(window, cx);
                        }
                    }

                    cx.stop_propagation();
                    window.refresh();
                });

                window.on_key_event({
                    let key_selection = selection_state.clone();
                    let key_id = selection_id.clone();
                    let key_text = raw_text.clone();
                    let key_group = selection_group.clone();
                    move |event: &KeyDownEvent, phase, _window, cx| {
                        if phase != DispatchPhase::Bubble || !is_active_text_target(&key_id) {
                            return;
                        }

                        let modifiers = event.keystroke.modifiers;
                        let platform_only = platform_primary_modifier(modifiers);
                        match event.keystroke.key.as_str() {
                            "a" if platform_only && !key_text.is_empty() => {
                                if let Some(group) = key_group.as_ref() {
                                    select_all_group_text(group);
                                } else {
                                    key_selection.borrow_mut().select_all(key_text.len());
                                }
                                cx.stop_propagation();
                            }
                            "c" if platform_only => {
                                if let Some(group) = key_group.as_ref() {
                                    if let Some(text) = selected_group_text(group) {
                                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                                        cx.stop_propagation();
                                        return;
                                    }
                                }
                                if let Some(range) = key_selection.borrow().selection_range() {
                                    if !range.is_empty() {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            key_text[range].to_string(),
                                        ));
                                        cx.stop_propagation();
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                });

                let hovered_index = selection_state.borrow().hovered_index;
                if let Some(index) = hovered_index {
                    if cursor_click_ranges
                        .iter()
                        .any(|range| range.contains(&index))
                        && (!click_requires_platform_modifier
                            || platform_click_modifier(window.modifiers()))
                    {
                        window.set_cursor_style(gpui::CursorStyle::PointingHand, hitbox);
                    } else {
                        window.set_cursor_style(gpui::CursorStyle::IBeam, hitbox);
                    }
                } else if hitbox.is_hovered(window) {
                    window.set_cursor_style(gpui::CursorStyle::IBeam, hitbox);
                }

                if let Some(range) = selection_group
                    .as_ref()
                    .and_then(|group| group_text_row_selection_range(group, raw_text.len()))
                    .or_else(|| selection_state.borrow().selection_range())
                {
                    for quad in selection_quads_for_range(
                        raw_text.as_ref(),
                        &text_layout,
                        range,
                        self.selection_color,
                    ) {
                        window.paint_quad(quad);
                    }
                }

                self.text
                    .paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);

                ((), selectable_state)
            },
        );
    }
}

impl IntoElement for SelectableText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[doc(hidden)]
#[derive(Default)]
pub struct AppTextInputState {
    focus_handle: Option<FocusHandle>,
    selection: Rc<RefCell<TextSelectionState>>,
}

pub struct AppTextInput {
    element_id: ElementId,
    selection_id: String,
    state: gpui::Entity<AppState>,
    field: AppTextFieldKind,
    placeholder: SharedString,
    text: StyledText,
    raw_text: SharedString,
    display_text: SharedString,
    autofocus: bool,
    multiline: bool,
    selection_color: gpui::Rgba,
}

impl AppTextInput {
    pub fn new(
        id: impl Into<SharedString>,
        state: gpui::Entity<AppState>,
        field: AppTextFieldKind,
        placeholder: impl Into<SharedString>,
    ) -> Self {
        let selection_id: SharedString = id.into();
        let element_id = ElementId::Name(selection_id.clone());
        let placeholder = placeholder.into();

        Self {
            element_id,
            selection_id: selection_id.to_string(),
            state,
            field,
            placeholder: placeholder.clone(),
            text: StyledText::new(SharedString::new("")),
            raw_text: SharedString::new(""),
            display_text: placeholder,
            autofocus: false,
            multiline: matches!(
                field,
                AppTextFieldKind::ReviewBody | AppTextFieldKind::InlineCommentDraft
            ),
            selection_color: accent_muted(),
        }
    }

    pub fn autofocus(mut self, autofocus: bool) -> Self {
        self.autofocus = autofocus;
        self
    }

    fn sync_content(&mut self, cx: &App) {
        let raw_text = {
            let app_state = self.state.read(cx);
            match self.field {
                AppTextFieldKind::PaletteQuery => app_state.palette_query.clone(),
                AppTextFieldKind::ReviewBody => app_state.review_body.clone(),
                AppTextFieldKind::WaymarkDraft => app_state.waymark_draft.clone(),
                AppTextFieldKind::InlineCommentDraft => app_state.inline_comment_draft.clone(),
                AppTextFieldKind::WaypointSpotlightQuery => {
                    app_state.waypoint_spotlight_query.clone()
                }
            }
        };

        self.raw_text = raw_text.clone().into();
        self.display_text = if raw_text.is_empty() {
            self.placeholder.clone()
        } else {
            self.raw_text.clone()
        };
        self.text = StyledText::new(self.display_text.clone());
    }
}

impl Element for AppTextInput {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.element_id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.sync_content(cx);
        self.text.request_layout(None, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        window.with_optional_element_state::<AppTextInputState, _>(
            global_id,
            |input_state, window| {
                let mut input_state =
                    input_state.map(|input_state| input_state.unwrap_or_default());
                if let Some(input_state) = input_state.as_mut() {
                    let focus_handle = input_state
                        .focus_handle
                        .get_or_insert_with(|| cx.focus_handle())
                        .clone();
                    window.set_focus_handle(&focus_handle, cx);
                }
                self.text
                    .prepaint(None, inspector_id, bounds, state, window, cx);
                let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                (hitbox, input_state)
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let text_layout = self.text.layout().clone();
        let raw_text = self.raw_text.clone();
        let selection_id = self.selection_id.clone();
        let field = self.field;
        let state = self.state.clone();
        let multiline = self.multiline;
        let autofocus = self.autofocus;

        window.with_element_state::<AppTextInputState, _>(
            global_id.unwrap(),
            |input_state, window| {
                let input_state = input_state.unwrap_or_default();
                let focus_handle = input_state.focus_handle.clone();
                let selection_state = input_state.selection.clone();
                selection_state.borrow_mut().clamp(raw_text.len());

                if autofocus {
                    if let Some(focus_handle) = focus_handle.as_ref() {
                        focus_handle.focus(window);
                    }
                    set_active_text_target(selection_id.clone());
                }

                let clear_hitbox = hitbox.clone();
                let clear_selection_state = selection_state.clone();
                let clear_selection_id = selection_id.clone();
                window.on_mouse_event(move |_event: &MouseDownEvent, phase, window, _cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if !is_active_text_target(&clear_selection_id)
                        || clear_hitbox.is_hovered(window)
                    {
                        return;
                    }

                    clear_active_text_target(&clear_selection_id);
                    clear_selection_state.borrow_mut().clear();
                    window.refresh();
                });

                let mouse_down_hitbox = hitbox.clone();
                let mouse_down_layout = text_layout.clone();
                let mouse_down_selection = selection_state.clone();
                let mouse_down_state = state.clone();
                let mouse_down_id = selection_id.clone();
                let mouse_down_text = raw_text.clone();
                let mouse_down_focus = focus_handle.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture
                        || event.button != MouseButton::Left
                        || !mouse_down_hitbox.is_hovered(window)
                    {
                        return;
                    }

                    let index = if mouse_down_text.is_empty() {
                        0
                    } else {
                        mouse_down_layout
                            .index_for_position(event.position)
                            .unwrap_or_else(|index| index)
                    };

                    mouse_down_state.update(cx, |app_state, cx| {
                        if matches!(field, AppTextFieldKind::ReviewBody) {
                            app_state.review_editor_active = true;
                            app_state.review_message = None;
                            app_state.review_success = false;
                        }
                        cx.notify();
                    });

                    {
                        let mut selection = mouse_down_selection.borrow_mut();
                        selection.mouse_down_index = Some(index);
                        selection.selecting = true;
                        if event.modifiers.shift && selection.anchor_index.is_some() {
                            selection.select_to(index);
                        } else {
                            selection.collapse_to(index);
                        }
                    }

                    if let Some(focus_handle) = mouse_down_focus.as_ref() {
                        focus_handle.focus(window);
                    }
                    set_active_text_target(mouse_down_id.clone());
                    cx.stop_propagation();
                    window.refresh();
                });

                let mouse_move_layout = text_layout.clone();
                let mouse_move_selection = selection_state.clone();
                let mouse_move_text = raw_text.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _cx| {
                    if phase != DispatchPhase::Bubble || !mouse_move_selection.borrow().selecting {
                        return;
                    }

                    let index = if mouse_move_text.is_empty() {
                        0
                    } else {
                        mouse_move_layout
                            .index_for_position(event.position)
                            .unwrap_or_else(|index| index)
                    };

                    mouse_move_selection.borrow_mut().select_to(index);
                    window.refresh();
                });

                let mouse_up_selection = selection_state.clone();
                window.on_mouse_event(move |_event: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if !mouse_up_selection.borrow().selecting {
                        return;
                    }

                    let mut selection = mouse_up_selection.borrow_mut();
                    selection.selecting = false;
                    selection.mouse_down_index = None;
                    drop(selection);

                    cx.stop_propagation();
                    window.refresh();
                });

                window.on_key_event({
                    let key_state = state.clone();
                    let key_selection = selection_state.clone();
                    let key_id = selection_id.clone();
                    let key_text = raw_text.clone();
                    move |event: &KeyDownEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble || !is_active_text_target(&key_id) {
                            return;
                        }

                        let modifiers = event.keystroke.modifiers;
                        let platform_only = platform_primary_modifier(modifiers);
                        let key = event.keystroke.key.as_str();

                        let mut handled = true;
                        match key {
                            "left" => {
                                key_state.update(cx, |app_state, cx| {
                                    move_input_selection(
                                        input_text_for_field(app_state, field),
                                        &key_selection,
                                        MoveDirection::Left,
                                        modifiers.shift,
                                    );
                                    cx.notify();
                                });
                            }
                            "right" => {
                                key_state.update(cx, |app_state, cx| {
                                    move_input_selection(
                                        input_text_for_field(app_state, field),
                                        &key_selection,
                                        MoveDirection::Right,
                                        modifiers.shift,
                                    );
                                    cx.notify();
                                });
                            }
                            "home" => {
                                key_selection
                                    .borrow_mut()
                                    .select_to_or_collapse(0, modifiers.shift);
                                window.refresh();
                            }
                            "end" => {
                                let len = key_text.len();
                                key_selection
                                    .borrow_mut()
                                    .select_to_or_collapse(len, modifiers.shift);
                                window.refresh();
                            }
                            "backspace" => {
                                key_state.update(cx, |app_state, cx| {
                                    edit_input_text(
                                        app_state,
                                        field,
                                        &key_selection,
                                        EditCommand::Backspace,
                                    );
                                    cx.notify();
                                });
                            }
                            "delete" => {
                                key_state.update(cx, |app_state, cx| {
                                    edit_input_text(
                                        app_state,
                                        field,
                                        &key_selection,
                                        EditCommand::Delete,
                                    );
                                    cx.notify();
                                });
                            }
                            "a" if platform_only => {
                                key_selection.borrow_mut().select_all(key_text.len());
                                window.refresh();
                            }
                            "c" if platform_only => {
                                if let Some(range) = key_selection.borrow().selection_range() {
                                    if !range.is_empty() {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            key_text[range].to_string(),
                                        ));
                                    }
                                }
                            }
                            "x" if platform_only => {
                                key_state.update(cx, |app_state, cx| {
                                    cut_input_text(app_state, field, &key_selection, cx);
                                    cx.notify();
                                });
                            }
                            "v" if platform_only => {
                                if let Some(text) =
                                    cx.read_from_clipboard().and_then(|item| item.text())
                                {
                                    key_state.update(cx, |app_state, cx| {
                                        edit_input_text(
                                            app_state,
                                            field,
                                            &key_selection,
                                            EditCommand::Insert(normalize_paste(field, &text)),
                                        );
                                        cx.notify();
                                    });
                                }
                            }
                            "enter" if !platform_only && multiline => {
                                key_state.update(cx, |app_state, cx| {
                                    edit_input_text(
                                        app_state,
                                        field,
                                        &key_selection,
                                        EditCommand::Insert("\n".to_string()),
                                    );
                                    cx.notify();
                                });
                            }
                            _ => {
                                handled = false;
                                if let Some(input) = event.keystroke.key_char.as_ref() {
                                    if should_insert_text(input, multiline) {
                                        let input = if multiline {
                                            input.to_string()
                                        } else {
                                            input.replace('\n', " ")
                                        };
                                        key_state.update(cx, |app_state, cx| {
                                            edit_input_text(
                                                app_state,
                                                field,
                                                &key_selection,
                                                EditCommand::Insert(input.clone()),
                                            );
                                            cx.notify();
                                        });
                                        handled = true;
                                    }
                                }
                            }
                        }

                        if handled {
                            cx.stop_propagation();
                        }
                    }
                });

                if hitbox.is_hovered(window) || is_active_text_target(&selection_id) {
                    window.set_cursor_style(gpui::CursorStyle::IBeam, hitbox);
                }

                if let Some(range) = selection_state.borrow().selection_range() {
                    for quad in selection_quads_for_range(
                        raw_text.as_ref(),
                        &text_layout,
                        range,
                        self.selection_color,
                    ) {
                        window.paint_quad(quad);
                    }
                }

                let cursor_quad = is_active_text_target(&selection_id)
                    .then(|| {
                        cursor_quad_for_index(&text_layout, selection_state.borrow().cursor_index())
                    })
                    .flatten();

                self.text
                    .paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);

                if let Some(cursor_quad) = cursor_quad {
                    window.paint_quad(cursor_quad);
                }

                ((), input_state)
            },
        );
    }
}

impl IntoElement for AppTextInput {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl TextSelectionState {
    fn select_to_or_collapse(&mut self, index: usize, extend: bool) {
        if extend {
            self.select_to(index);
        } else {
            self.collapse_to(index);
        }
    }
}

#[derive(Clone, Copy)]
enum MoveDirection {
    Left,
    Right,
}

enum EditCommand {
    Insert(String),
    Backspace,
    Delete,
}

fn move_input_selection(
    text: &str,
    selection: &Rc<RefCell<TextSelectionState>>,
    direction: MoveDirection,
    extend: bool,
) {
    let mut selection = selection.borrow_mut();
    selection.clamp(text.len());

    let target = if extend {
        match direction {
            MoveDirection::Left => previous_boundary(text, selection.cursor_index()),
            MoveDirection::Right => next_boundary(text, selection.cursor_index()),
        }
    } else if let Some(range) = selection.selection_range() {
        if !range.is_empty() {
            match direction {
                MoveDirection::Left => range.start,
                MoveDirection::Right => range.end,
            }
        } else {
            match direction {
                MoveDirection::Left => previous_boundary(text, selection.cursor_index()),
                MoveDirection::Right => next_boundary(text, selection.cursor_index()),
            }
        }
    } else {
        match direction {
            MoveDirection::Left => previous_boundary(text, selection.cursor_index()),
            MoveDirection::Right => next_boundary(text, selection.cursor_index()),
        }
    };

    selection.select_to_or_collapse(target, extend);
}

fn input_text_for_field<'a>(state: &'a AppState, field: AppTextFieldKind) -> &'a str {
    match field {
        AppTextFieldKind::PaletteQuery => state.palette_query.as_str(),
        AppTextFieldKind::ReviewBody => state.review_body.as_str(),
        AppTextFieldKind::WaymarkDraft => state.waymark_draft.as_str(),
        AppTextFieldKind::InlineCommentDraft => state.inline_comment_draft.as_str(),
        AppTextFieldKind::WaypointSpotlightQuery => state.waypoint_spotlight_query.as_str(),
    }
}

fn set_input_text_for_field(state: &mut AppState, field: AppTextFieldKind, value: String) {
    match field {
        AppTextFieldKind::PaletteQuery => {
            state.palette_query = value;
            state.palette_selected_index = 0;
            state.palette_scroll_animation_generation =
                state.palette_scroll_animation_generation.wrapping_add(1);
            state.palette_scroll_animation_active = false;
            state.palette_last_scroll_navigation_at = None;
            state
                .palette_scroll_handle
                .set_offset(point(px(0.0), px(0.0)));
        }
        AppTextFieldKind::ReviewBody => {
            state.review_body = value;
        }
        AppTextFieldKind::WaymarkDraft => {
            state.waymark_draft = value;
        }
        AppTextFieldKind::InlineCommentDraft => {
            state.inline_comment_draft = value;
        }
        AppTextFieldKind::WaypointSpotlightQuery => {
            state.waypoint_spotlight_query = value;
            state.waypoint_spotlight_selected_index = 0;
        }
    }
}

fn cut_input_text(
    state: &mut AppState,
    field: AppTextFieldKind,
    selection: &Rc<RefCell<TextSelectionState>>,
    cx: &mut App,
) {
    let text = input_text_for_field(state, field).to_string();
    let range = selection.borrow().selection_range().unwrap_or(0..0);
    if range.is_empty() {
        return;
    }

    cx.write_to_clipboard(ClipboardItem::new_string(text[range.clone()].to_string()));
    apply_replacement(state, field, selection, range, "");
}

fn edit_input_text(
    state: &mut AppState,
    field: AppTextFieldKind,
    selection: &Rc<RefCell<TextSelectionState>>,
    command: EditCommand,
) {
    let text = input_text_for_field(state, field).to_string();
    let mut selection_state = selection.borrow_mut();
    selection_state.clamp(text.len());

    let selection_range = selection_state.selection_range().unwrap_or_else(|| {
        let cursor = selection_state.cursor_index();
        cursor..cursor
    });

    match command {
        EditCommand::Insert(new_text) => {
            drop(selection_state);
            apply_replacement(state, field, selection, selection_range, &new_text);
        }
        EditCommand::Backspace => {
            let delete_range = if selection_range.is_empty() {
                let cursor = selection_range.end;
                previous_boundary(&text, cursor)..cursor
            } else {
                selection_range
            };
            drop(selection_state);
            apply_replacement(state, field, selection, delete_range, "");
        }
        EditCommand::Delete => {
            let delete_range = if selection_range.is_empty() {
                let cursor = selection_range.end;
                cursor..next_boundary(&text, cursor)
            } else {
                selection_range
            };
            drop(selection_state);
            apply_replacement(state, field, selection, delete_range, "");
        }
    }
}

fn apply_replacement(
    state: &mut AppState,
    field: AppTextFieldKind,
    selection: &Rc<RefCell<TextSelectionState>>,
    range: Range<usize>,
    replacement: &str,
) {
    let text = input_text_for_field(state, field).to_string();
    let mut next = String::with_capacity(text.len() + replacement.len());
    next.push_str(&text[..range.start]);
    next.push_str(replacement);
    next.push_str(&text[range.end..]);
    set_input_text_for_field(state, field, next);

    let cursor = range.start + replacement.len();
    let mut selection_state = selection.borrow_mut();
    selection_state.collapse_to(cursor);
    selection_state.clamp(input_text_for_field(state, field).len());
}

fn normalize_paste(field: AppTextFieldKind, text: &str) -> String {
    match field {
        AppTextFieldKind::PaletteQuery => text.replace('\n', " "),
        AppTextFieldKind::ReviewBody => text.to_string(),
        AppTextFieldKind::WaymarkDraft => text.replace('\n', " "),
        AppTextFieldKind::InlineCommentDraft => text.to_string(),
        AppTextFieldKind::WaypointSpotlightQuery => text.replace('\n', " "),
    }
}

fn should_insert_text(input: &str, multiline: bool) -> bool {
    if input == "\t" {
        return false;
    }
    if input == "\n" {
        return multiline;
    }
    !input.chars().all(|character| character.is_control())
}

fn selection_quads_for_range(
    text: &str,
    layout: &TextLayout,
    selection: Range<usize>,
    color: gpui::Rgba,
) -> Vec<PaintQuad> {
    if selection.is_empty() || text.is_empty() {
        return Vec::new();
    }

    let bounds = layout.bounds();
    let line_height = layout.line_height();
    let hard_lines = text.split('\n').collect::<Vec<_>>();
    let mut line_start = 0usize;
    let mut block_y = Pixels::ZERO;
    let mut quads = Vec::new();

    for (line_ix, line_text) in hard_lines.iter().enumerate() {
        let line_query_index = line_start.min(layout.len().saturating_sub(1));
        let Some(line_layout) = layout.line_layout_for_index(line_query_index) else {
            line_start += line_text.len();
            if line_ix + 1 < hard_lines.len() {
                line_start += 1;
            }
            continue;
        };

        let segment_ends = wrapped_segment_end_indices(&line_layout);
        let mut segment_start = 0usize;
        for segment_end in segment_ends {
            let global_segment_start = line_start + segment_start;
            let global_segment_end = line_start + segment_end;
            let overlap_start = selection.start.max(global_segment_start);
            let overlap_end = selection.end.min(global_segment_end);
            if overlap_start < overlap_end {
                let local_start = overlap_start - line_start;
                let local_end = overlap_end - line_start;
                if let (Some(start), Some(end)) = (
                    line_layout.position_for_index(local_start, line_height),
                    line_layout.position_for_index(local_end, line_height),
                ) {
                    let top = bounds.top() + block_y + start.y;
                    let bottom = top + line_height;
                    let left = bounds.left() + start.x;
                    let right = bounds.left() + end.x.max(start.x + px(1.0));
                    quads.push(fill(
                        Bounds::from_corners(point(left, top), point(right, bottom)),
                        color,
                    ));
                }
            }
            segment_start = segment_end;
        }

        block_y += line_layout.size(line_height).height;
        line_start += line_text.len();
        if line_ix + 1 < hard_lines.len() {
            line_start += 1;
        }
    }

    quads
}

fn wrapped_segment_end_indices(layout: &WrappedLineLayout) -> Vec<usize> {
    let mut ends = layout
        .wrap_boundaries()
        .iter()
        .map(|boundary| {
            let run = &layout.runs()[boundary.run_ix];
            let glyph = &run.glyphs[boundary.glyph_ix];
            glyph.index
        })
        .collect::<Vec<_>>();
    ends.push(layout.len());
    ends
}

fn cursor_quad_for_index(layout: &TextLayout, index: usize) -> Option<PaintQuad> {
    let position = layout.position_for_index(index)?;
    let line_height = layout.line_height();
    Some(cursor_quad_from_position(position, line_height))
}

fn cursor_quad_from_position(position: gpui::Point<Pixels>, line_height: Pixels) -> PaintQuad {
    fill(Bounds::new(position, size(px(2.0), line_height)), accent())
}

fn previous_boundary(text: &str, offset: usize) -> usize {
    text.char_indices()
        .rev()
        .find_map(|(index, _)| (index < offset).then_some(index))
        .unwrap_or(0)
}

fn next_boundary(text: &str, offset: usize) -> usize {
    text.char_indices()
        .find_map(|(index, _)| (index > offset).then_some(index))
        .unwrap_or(text.len())
}

fn platform_primary_modifier(modifiers: gpui::Modifiers) -> bool {
    shortcuts::secondary_text_modifier(modifiers)
}

fn platform_click_modifier(modifiers: gpui::Modifiers) -> bool {
    shortcuts::secondary_plain_modifier(modifiers)
}

fn register_group_text_row(group: &GroupTextSelectionConfig, text: &str) {
    TEXT_SELECTION_GROUPS.with(|groups| {
        groups
            .borrow_mut()
            .entry(group.group_id.clone())
            .or_default()
            .rows
            .insert(group.row_order, text.to_string());
    });
}

fn start_group_text_selection(group: &GroupTextSelectionConfig, index: usize, extend: bool) {
    TEXT_SELECTION_GROUPS.with(|groups| {
        let mut groups = groups.borrow_mut();
        let state = groups.entry(group.group_id.clone()).or_default();
        let point = GroupTextPoint {
            row_order: group.row_order,
            index,
        };
        if extend && state.anchor.is_some() {
            state.head = Some(point);
        } else {
            state.anchor = Some(point);
            state.head = Some(point);
        }
        state.mouse_down = Some(point);
        state.selecting = true;
    });
}

fn update_group_text_selection(group: &GroupTextSelectionConfig, index: usize) {
    TEXT_SELECTION_GROUPS.with(|groups| {
        let mut groups = groups.borrow_mut();
        let state = groups.entry(group.group_id.clone()).or_default();
        if state.anchor.is_none() {
            state.anchor = Some(GroupTextPoint {
                row_order: group.row_order,
                index,
            });
        }
        state.head = Some(GroupTextPoint {
            row_order: group.row_order,
            index,
        });
    });
}

fn finish_group_text_selection(group: &GroupTextSelectionConfig) {
    TEXT_SELECTION_GROUPS.with(|groups| {
        if let Some(state) = groups.borrow_mut().get_mut(&group.group_id) {
            state.selecting = false;
            state.mouse_down = None;
        }
    });
}

fn clear_group_text_selection(group: &GroupTextSelectionConfig) {
    TEXT_SELECTION_GROUPS.with(|groups| {
        if let Some(state) = groups.borrow_mut().get_mut(&group.group_id) {
            state.anchor = None;
            state.head = None;
            state.mouse_down = None;
            state.selecting = false;
        }
    });
}

fn is_group_text_selecting(group: &GroupTextSelectionConfig) -> bool {
    TEXT_SELECTION_GROUPS.with(|groups| {
        groups
            .borrow()
            .get(&group.group_id)
            .map(|state| state.selecting)
            .unwrap_or(false)
    })
}

fn select_all_group_text(group: &GroupTextSelectionConfig) {
    TEXT_SELECTION_GROUPS.with(|groups| {
        let mut groups = groups.borrow_mut();
        let state = groups.entry(group.group_id.clone()).or_default();
        let Some((&first_order, _)) = state.rows.first_key_value() else {
            return;
        };
        let Some((&last_order, last_text)) = state.rows.last_key_value() else {
            return;
        };
        state.anchor = Some(GroupTextPoint {
            row_order: first_order,
            index: 0,
        });
        state.head = Some(GroupTextPoint {
            row_order: last_order,
            index: last_text.len(),
        });
        state.mouse_down = None;
        state.selecting = false;
    });
}

fn group_text_row_selection_range(
    group: &GroupTextSelectionConfig,
    row_len: usize,
) -> Option<Range<usize>> {
    TEXT_SELECTION_GROUPS.with(|groups| {
        let groups = groups.borrow();
        let state = groups.get(&group.group_id)?;
        let (start, end) = ordered_group_text_points(state)?;
        selection_range_for_group_row(start, end, group.row_order, row_len)
    })
}

fn selected_group_text(group: &GroupTextSelectionConfig) -> Option<String> {
    TEXT_SELECTION_GROUPS.with(|groups| {
        let groups = groups.borrow();
        let state = groups.get(&group.group_id)?;
        let (start, end) = ordered_group_text_points(state)?;
        let mut lines = Vec::new();
        for (&row_order, text) in state.rows.range(start.row_order..=end.row_order) {
            let range =
                selection_range_for_group_row(start, end, row_order, text.len()).unwrap_or(0..0);
            lines.push(text.get(range).unwrap_or_default().to_string());
        }
        let selected = lines.join("\n");
        (!selected.is_empty()).then_some(selected)
    })
}

fn ordered_group_text_points(
    state: &GroupTextSelectionState,
) -> Option<(GroupTextPoint, GroupTextPoint)> {
    let anchor = state.anchor?;
    let head = state.head.unwrap_or(anchor);
    if anchor <= head {
        Some((anchor, head))
    } else {
        Some((head, anchor))
    }
}

fn selection_range_for_group_row(
    start: GroupTextPoint,
    end: GroupTextPoint,
    row_order: i64,
    row_len: usize,
) -> Option<Range<usize>> {
    if start == end || row_order < start.row_order || row_order > end.row_order {
        return None;
    }

    let range = if start.row_order == end.row_order {
        start.index.min(row_len)..end.index.min(row_len)
    } else if row_order == start.row_order {
        start.index.min(row_len)..row_len
    } else if row_order == end.row_order {
        0..end.index.min(row_len)
    } else {
        0..row_len
    };

    (!range.is_empty()).then_some(range)
}

fn set_active_text_target(id: String) {
    ACTIVE_TEXT_TARGET.with(|active| {
        active.replace(Some(id));
    });
}

fn clear_active_text_target(id: &str) {
    ACTIVE_TEXT_TARGET.with(|active| {
        let should_clear = active.borrow().as_deref() == Some(id);
        if should_clear {
            active.replace(None);
        }
    });
}

fn is_active_text_target(id: &str) -> bool {
    ACTIVE_TEXT_TARGET.with(|active| active.borrow().as_deref() == Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_quad_uses_absolute_layout_position() {
        let quad = cursor_quad_from_position(point(px(120.0), px(42.0)), px(18.0));

        assert_eq!(quad.bounds.left(), px(120.0));
        assert_eq!(quad.bounds.top(), px(42.0));
        assert_eq!(quad.bounds.size, size(px(2.0), px(18.0)));
    }

    #[test]
    fn selected_group_text_joins_ordered_row_ranges() {
        let group_id = "selected-group-text-joins-ordered-row-ranges";
        let first = GroupTextSelectionConfig {
            group_id: group_id.to_string(),
            row_order: 0,
        };
        let middle = GroupTextSelectionConfig {
            group_id: group_id.to_string(),
            row_order: 1,
        };
        let last = GroupTextSelectionConfig {
            group_id: group_id.to_string(),
            row_order: 2,
        };

        register_group_text_row(&first, "foo");
        register_group_text_row(&middle, "bar");
        register_group_text_row(&last, "baz");
        start_group_text_selection(&first, 1, false);
        update_group_text_selection(&last, 2);
        finish_group_text_selection(&last);

        assert_eq!(selected_group_text(&first), Some("oo\nbar\nba".to_string()));
    }
}
