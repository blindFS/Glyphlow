use crate::{
    config::{
        CustomTarget, GlyphlowConfig, GlyphlowTheme, RoleOfInterest, VisibilityCheckingLevel,
    },
    util::{Frame, HintBox, hint_boxes_from_frames, select_range_helper},
};
use accessibility::{AXAttribute, AXUIElement, AXUIElementAttributes};
use accessibility_sys::{
    AXUIElementCopyMultipleAttributeValues, AXValueCreate, AXValueGetValue, AXValueRef,
    kAXButtonRole, kAXCellRole, kAXCheckBoxRole, kAXComboBoxRole, kAXContentListSubrole,
    kAXErrorSuccess, kAXGroupRole, kAXHiddenAttribute, kAXImageRole, kAXListRole, kAXMenuItemRole,
    kAXPopUpButtonRole, kAXPositionAttribute, kAXPressAction, kAXRoleAttribute, kAXRowRole,
    kAXScrollAreaRole, kAXScrollBarRole, kAXSelectedTextRangeAttribute, kAXSizeAttribute,
    kAXStaticTextRole, kAXTextAreaRole, kAXTextFieldRole, kAXTitleAttribute, kAXValueTypeCFRange,
    kAXValueTypeCGPoint, kAXValueTypeCGSize, kAXWindowRole,
};
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFRange, CFType, CFTypeRef, FromVoid, TCFType},
    boolean::CFBoolean,
    string::CFString,
};
use objc2_core_foundation::{CGPoint, CGSize};
use std::collections::HashMap;

const BASIC_ATTRIBUTES: [&str; 4] = [
    kAXRoleAttribute,
    kAXPositionAttribute,
    kAXSizeAttribute,
    kAXHiddenAttribute,
];

struct ElementBasicAttributes {
    pub frame: Option<Frame>,
    pub hidden: bool,
    pub role: String,
}

impl ElementBasicAttributes {
    fn visible_frame(&self, parent_frame: &Frame) -> Option<Frame> {
        // NOTE: scroll bar positioning depends on its value
        if self.role == kAXScrollBarRole {
            return Some(*parent_frame);
        }

        if self.hidden {
            return None;
        }

        // TODO: handle edge cases according to role
        // e.g. popup menu
        if let Some(this_frame) = self.frame {
            // TODO: For some fully visible structure of A -> B -> C,
            // somehow the intersection of either A and B or B and C is not empty,
            // but the intersection of all those 3 is empty.
            // An extra mode that dives elements 1 level at a time, instead of flattening them all at once
            // TODO: trade-off among false-positive, false-negative and performance
            this_frame.intersect(parent_frame)
        } else {
            Some(*parent_frame)
        }
    }

    fn from(element: &AXUIElement) -> Option<Self> {
        let cf_attributes: Vec<CFString> =
            BASIC_ATTRIBUTES.iter().map(|&s| CFString::new(s)).collect();
        let cf_array_in = CFArray::from_CFTypes(&cf_attributes);

        let mut values_ref: CFArrayRef = std::ptr::null();
        let err = unsafe {
            AXUIElementCopyMultipleAttributeValues(
                element.as_concrete_TypeRef(),
                cf_array_in.as_concrete_TypeRef(),
                // Don't stop on error
                0,
                &mut values_ref,
            )
        };

        if err != kAXErrorSuccess || values_ref.is_null() {
            None
        } else {
            let values_array: CFArray<CFType> =
                unsafe { CFArray::wrap_under_create_rule(values_ref) };
            let values = values_array.get_all_values();

            let role_cf = values_array.get(0).and_then(|v| v.downcast::<CFString>())?;
            let role = role_cf.to_string();

            let pos = values
                .get(1)
                .and_then(|pos_ptr| cftype_to_rust_type::<CGPoint>(*pos_ptr, kAXValueTypeCGPoint));

            let size = values
                .get(2)
                .and_then(|size_ptr| cftype_to_rust_type::<CGSize>(*size_ptr, kAXValueTypeCGSize));

            let frame = match (pos, size) {
                (Some(p), Some(s)) => Some(Frame::new(p.x, p.y, p.x + s.width, p.y + s.height)),
                _ => None,
            };

            let hidden = values_array
                .get(3)
                .and_then(|v| v.downcast::<CFBoolean>())
                .map(bool::from)
                .unwrap_or_default();

            Some(Self {
                role,
                frame,
                hidden,
            })
        }
    }

    fn match_custom_target(&self, target: &CustomTarget) -> bool {
        if let Some(size) = target.size
            && !self.frame.is_some_and(|f| f.size() == size)
        {
            return false;
        }
        let role = self.role.to_lowercase();
        let target_role = target.role.to_lowercase();
        target_role.split('|').any(|r| role.contains(r))
    }
}

#[derive(Clone, Debug)]
pub enum ElementKind {
    Standard {
        element: AXUIElement,
        role: RoleOfInterest,
    },
    Pseudo,
}

#[derive(Clone, Debug)]
pub struct ElementOfInterest {
    pub kind: ElementKind,
    pub context: Option<String>,
    pub frame: Frame,
}

impl ElementOfInterest {
    pub fn new(
        element: AXUIElement,
        context: Option<String>,
        role: RoleOfInterest,
        frame: Frame,
    ) -> Self {
        Self {
            kind: ElementKind::Standard { element, role },
            context,
            frame,
        }
    }

    pub fn pseudo(context: Option<String>, frame: Frame) -> Self {
        Self {
            kind: ElementKind::Pseudo,
            context,
            frame,
        }
    }

    pub fn role(&self) -> RoleOfInterest {
        match &self.kind {
            ElementKind::Standard { role, .. } => *role,
            ElementKind::Pseudo => RoleOfInterest::PseudoText,
        }
    }

    pub fn element(&self) -> Option<&AXUIElement> {
        match &self.kind {
            ElementKind::Standard { element, .. } => Some(element),
            ElementKind::Pseudo => None,
        }
    }
}

#[derive(Default)]
pub struct ElementCache {
    pub cache: Vec<ElementOfInterest>,
    seen_center: HashMap<(u64, u64), usize>,
    element_min_width: f64,
    element_min_height: f64,
    image_min_size: f64,
}

impl ElementCache {
    pub fn new(min_width: f64, min_height: f64, image_min_size: f64) -> Self {
        ElementCache {
            cache: vec![],
            seen_center: HashMap::new(),
            element_min_width: min_width,
            element_min_height: min_height,
            image_min_size,
        }
    }

    pub fn reload_config(&mut self, new_config: &GlyphlowConfig) {
        self.element_min_width = new_config.element_min_width as f64;
        self.element_min_height = new_config.element_min_height as f64;
        self.image_min_size = new_config.image_min_size as f64;
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.seen_center.clear();
    }

    pub fn force_add(
        &mut self,
        element: &AXUIElement,
        context: Option<String>,
        role: RoleOfInterest,
        frame: Option<Frame>,
    ) {
        // Has to be concrete leaf elements with frames
        let Some(frame) = frame else {
            return;
        };

        let (x, y) = frame.center();
        // f64 to u64 for hashing
        let center = (x.to_bits(), y.to_bits());

        let new_ele = ElementOfInterest::new(element.clone(), context, role, frame);
        self.seen_center.insert(center, self.cache.len());
        self.cache.push(new_ele);
    }

    pub fn add(
        &mut self,
        element: &AXUIElement,
        context: Option<String>,
        role: RoleOfInterest,
        frame: Option<Frame>,
    ) {
        // Has to be concrete leaf elements with frames
        let Some(frame) = frame else {
            return;
        };

        // NOTE: Use parent frames for scroll bars,
        // replaces the existing AXScrollArea in later center point check
        let frame = if role == RoleOfInterest::ScrollBar
            && let Some(parent_frame) = element
                .parent()
                .ok()
                .and_then(|p| ElementBasicAttributes::from(&p))
                .and_then(|p_fp| p_fp.frame)
        {
            parent_frame
        } else {
            frame
        };

        let (w, h) = frame.size();
        match role {
            // NOTE: some roles to keep
            RoleOfInterest::Generic | RoleOfInterest::ScrollBar | RoleOfInterest::CheckBox => {}
            // HACK: some menu items (like Apple Intelligence writing tools)
            // may have zero sized shadows, skip them to keep the workflow going
            RoleOfInterest::CustomTarget if w != 0.0 && h != 0.0 => {}
            RoleOfInterest::Image if w.min(h) < self.image_min_size => {
                return;
            }
            // Keep large enough images
            RoleOfInterest::Image => (),
            // Keep large enough text fields even if the text can be empty
            RoleOfInterest::TextField
                if (w < self.element_min_width || h < self.element_min_height) =>
            {
                return;
            }
            RoleOfInterest::TextField => (),
            // Check text before size, keep small texts
            _ if context.is_some() => {
                // Skip elements with empty/nonsense text
                if context.as_ref().is_some_and(|ctx| {
                    ctx.is_empty()
                        || ctx
                            .chars()
                            .all(|c| c.is_ascii_punctuation() || c.is_whitespace())
                }) {
                    return;
                }
            }
            _ if (w < self.element_min_width || h < self.element_min_height) => {
                return;
            }
            _ => (),
        }

        let (x, y) = frame.center();
        // f64 to u64 for hashing
        let center = (x.to_bits(), y.to_bits());

        // NOTE: de-duplication for DOM elements
        let new_ele = ElementOfInterest::new(element.clone(), context, role, frame);
        if let Some(idx) = self.seen_center.get(&center) {
            self.cache[*idx] = new_ele;
        } else {
            self.seen_center.insert(center, self.cache.len());
            self.cache.push(new_ele);
        }
    }

    pub fn hint_boxes(
        &self,
        screen_frame: &Frame,
        theme: &GlyphlowTheme,
        colored_frame_min_size: f64,
    ) -> (u32, Vec<HintBox>) {
        hint_boxes_from_frames(
            self.cache.len(),
            self.cache.iter().map(|it| it.frame),
            screen_frame,
            theme,
            colored_frame_min_size,
        )
    }

    pub fn select_range(
        &self,
        idx1: usize,
        idx2: usize,
        ref_role: Option<&RoleOfInterest>,
    ) -> Option<(String, Frame)> {
        let choices: Vec<(String, Frame, bool)> = self
            .cache
            .iter()
            .map(|eoi| {
                let is_valid = ref_role.is_none_or(|ref_role| *ref_role == eoi.role());
                (eoi.context.clone().unwrap_or_default(), eoi.frame, is_valid)
            })
            .collect();
        select_range_helper(&choices, idx1, idx2)
    }
}

fn role_to_interest(role: &str) -> RoleOfInterest {
    #[allow(non_upper_case_globals)]
    match role.to_string().as_str() {
        kAXImageRole => RoleOfInterest::Image,
        kAXTextFieldRole | kAXTextAreaRole | kAXComboBoxRole => RoleOfInterest::TextField,
        kAXMenuItemRole => RoleOfInterest::MenuItem,
        kAXPopUpButtonRole | kAXButtonRole | "AXRadioButton" => RoleOfInterest::Button,
        kAXCheckBoxRole => RoleOfInterest::CheckBox,
        kAXStaticTextRole | "AXHeading" => RoleOfInterest::StaticText,
        kAXScrollBarRole => RoleOfInterest::ScrollBar,
        _ => RoleOfInterest::Generic,
    }
}

pub trait GetAttribute {
    fn get_attribute(&self, attribute_name: &str) -> Option<CFType>;
    fn get_attribute_string(&self, attribute_name: &str) -> Option<String>;
    fn get_string_value_or_description(&self) -> Option<String>;
    fn get_frame(&self, default: Frame) -> Frame;
    fn get_dom_classes(&self) -> Option<Vec<String>>;
    fn inspect(&self) -> String;
    fn is_clickable(&self) -> bool;
    fn has_children(&self) -> bool;
    fn match_custom_target(&self, target: &CustomTarget) -> bool;
}

impl GetAttribute for AXUIElement {
    fn get_attribute(&self, attribute_name: &str) -> Option<CFType> {
        self.attribute(&AXAttribute::new(&CFString::new(attribute_name)))
            .ok()
    }

    fn get_attribute_string(&self, attribute_name: &str) -> Option<String> {
        self.get_attribute(attribute_name)
            .and_then(|val| val.downcast::<CFString>())
            .map(|cf| cf.to_string())
    }

    fn get_string_value_or_description(&self) -> Option<String> {
        self.value()
            .ok()
            .and_then(|v| v.downcast::<CFString>())
            .or_else(|| self.description().ok())
            .map(|cf| cf.to_string())
    }

    fn get_frame(&self, default_frame: Frame) -> Frame {
        let cf_array_in = CFArray::from_CFTypes(&[
            CFString::new(kAXPositionAttribute),
            CFString::new(kAXSizeAttribute),
        ]);

        let mut values_ref: CFArrayRef = std::ptr::null();
        let err = unsafe {
            AXUIElementCopyMultipleAttributeValues(
                self.as_concrete_TypeRef(),
                cf_array_in.as_concrete_TypeRef(),
                // Don't stop on error
                0,
                &mut values_ref,
            )
        };

        let frame = if err != kAXErrorSuccess || values_ref.is_null() {
            None
        } else {
            let values_array: CFArray<CFType> =
                unsafe { CFArray::wrap_under_create_rule(values_ref) };
            let values = values_array.get_all_values();

            let pos = values
                .first()
                .and_then(|pos_ptr| cftype_to_rust_type::<CGPoint>(*pos_ptr, kAXValueTypeCGPoint));

            let size = values
                .last()
                .and_then(|size_ptr| cftype_to_rust_type::<CGSize>(*size_ptr, kAXValueTypeCGSize));

            match (pos, size) {
                (Some(p), Some(s)) => {
                    Frame::new(p.x, p.y, p.x + s.width, p.y + s.height).intersect(&default_frame)
                }
                _ => None,
            }
        };
        frame.unwrap_or(default_frame)
    }

    fn inspect(&self) -> String {
        let Some(fp) = ElementBasicAttributes::from(self) else {
            return "Unknown".into();
        };

        let mut msg = String::new();

        msg.push_str(&format!("Role: {}\n", fp.role));

        if let Ok(subrole) = self.subrole() {
            msg.push_str(&format!("Subrole: {}\n", subrole));
        }

        if let Some(f) = fp.frame {
            let CGPoint { x, y } = f.top_left;
            msg.push_str(&format!("Pos: x: {x}, y: {y}\n"));
            let (w, h) = f.size();
            msg.push_str(&format!("Size: width: {w}, height: {h}\n"));
        }

        if let Ok(children) = self.children() {
            msg.push_str(&format!("Children num: {}\n", children.len()));
        }

        if let Ok(t) = self.title() {
            msg.push_str(&format!("Title: {t}\n"));
        }

        if let Ok(l) = self.label_value() {
            msg.push_str(&format!("Label: {l}\n"));
        }

        if let Ok(d) = self.description() {
            msg.push_str(&format!("Description: {d}\n"));
        }

        if let Ok(v) = self.value() {
            msg.push_str(&format!("Value: {v:?}\n"));
        }

        msg
        // for attr in &self.attribute_names().unwrap() {
        //     println!(
        //         "{role:?} - {attr:?} - {:?}",
        //         self.get_attribute(attr.to_string().as_str()),
        //     );
        // }
    }

    fn is_clickable(&self) -> bool {
        self.action_names().is_ok_and(|actions| {
            actions
                .iter()
                .any(|action| action.to_string() == kAXPressAction)
        })
    }

    fn has_children(&self) -> bool {
        self.children()
            .ok()
            .is_some_and(|children| !children.is_empty())
    }

    fn get_dom_classes(&self) -> Option<Vec<String>> {
        let cf_vals = self
            .get_attribute("AXDOMClassList")?
            .downcast::<CFArray>()?;

        let mut classes: Vec<String> = Vec::new();
        for val in cf_vals.iter() {
            let s = unsafe { CFString::from_void(*val) };
            classes.push(s.to_string());
        }

        Some(classes)
    }

    fn match_custom_target(&self, target: &CustomTarget) -> bool {
        if let Some(description) = target.description.as_ref()
            && !self.description().is_ok_and(|d| d == *description)
        {
            return false;
        }
        if let Some(title) = target.title.as_ref()
            && !self.title().is_ok_and(|t| t == *title)
        {
            return false;
        }
        if let Some(label) = target.label.as_ref()
            && !self.label_value().is_ok_and(|l| l == *label)
        {
            return false;
        }
        if let Some(value) = target.value.as_ref()
            && !self
                .value()
                .ok()
                .and_then(|v| v.downcast::<CFString>())
                .is_some_and(|v| v == *value)
        {
            return false;
        }
        true
    }
}

pub trait SetAttribute {
    fn set_attribute_by_name(&self, attribute_name: &str, value: CFType);
    fn set_selected_range(&self, location: isize, length: isize);
}

impl SetAttribute for AXUIElement {
    fn set_attribute_by_name(&self, attribute_name: &str, value: CFType) {
        let attr = AXAttribute::new(&CFString::new(attribute_name));
        if let Err(e) = self.set_attribute(&attr, value) {
            log::warn!("Failed to set attribute: {e}");
        };
    }

    fn set_selected_range(&self, location: isize, length: isize) {
        let range = CFRange::init(location, length);
        if let Some(wrapped_range) = rust_type_to_cftype(range, kAXValueTypeCFRange) {
            self.set_attribute_by_name(kAXSelectedTextRangeAttribute, wrapped_range);
        }
    }
}

/// A safe helper to extract C-structs from an AXValue stored inside a CFType.
fn cftype_to_rust_type<T: Default>(cf_type: CFTypeRef, value_type: u32) -> Option<T> {
    if cf_type.is_null() {
        return None;
    }

    unsafe {
        let value_ref = cf_type as AXValueRef;
        let mut result = T::default();

        AXValueGetValue(value_ref, value_type, &mut result as *mut T as *mut _).then_some(result)
    }
}

/// A helper for types have no impl Into<CFType>
fn rust_type_to_cftype<T>(value: T, value_type: u32) -> Option<CFType> {
    unsafe {
        let raw_value = AXValueCreate(value_type, &value as *const _ as *const std::ffi::c_void);
        if raw_value.is_null() {
            log::error!("Failed to create AXValue");
            return None;
        }

        Some(CFType::wrap_under_create_rule(raw_value as CFTypeRef))
    }
}

#[derive(Debug, Default, PartialEq, Clone)]
pub enum Target {
    #[default]
    Clickable,
    Image,
    ImageOCR,
    Editable,
    Edit,
    Text,
    MenuItem,
    ChildElement,
    Scrollable,
    Custom(CustomTarget),
}

const MAX_DEPTH: u8 = 200;

pub fn traverse_elements(
    element: &AXUIElement,
    parent_frame: &Frame,
    window_frame: &Frame,
    cache: &mut ElementCache,
    target: &Target,
    vis_level: VisibilityCheckingLevel,
    depth: u8,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let Some(ele_fp) = ElementBasicAttributes::from(element) else {
        return;
    };

    // WARN: Performance critical! Exclude electron elements scrolled off y axis,
    if ele_fp.frame.is_some_and(|f| {
        let (w, h) = f.size();
        (h == 0.0 && f.bottom_right.y == window_frame.bottom_right.y)
            // NOTE: keep full width elements, e.g. Brave google search
            || (h == 1.0 && f.top_left.y == window_frame.top_left.y && w != window_frame.size().0)
        // NOTE: should avoid false negatives of ancestors for some menu items,
        // e.g. (Discord right click menu)
    }) && vis_level != VisibilityCheckingLevel::Loosest
    {
        return;
    }

    // Get child elements 1 level lower
    // for false negatives aggressively filtered by the visibility checker
    if *target == Target::ChildElement {
        let Ok(children) = element.visible_children().or_else(|_| element.children()) else {
            return;
        };
        for child in &children {
            // NOTE: Some apps, like App Store, have circular referencing
            if *child == *element {
                continue;
            }
            if let Some(child_fp) = ElementBasicAttributes::from(&child)
                && let Some(c_f) = child_fp.frame
                && let Some(inter) = child_fp.visible_frame(window_frame)
            {
                // NOTE: recur into temp nodes with nonsense frames
                let (c_w, c_h) = c_f.size();
                if child_fp.role != kAXScrollBarRole && inter.contains(window_frame)
                    || c_w <= 1.0
                    || c_h <= 1.0
                    || (child_fp.role == kAXGroupRole && {
                        // Dominating child groups are usually meaningless
                        let (i_w, i_h) = inter.size();
                        let (w_w, w_h) = window_frame.size();
                        i_w > 0.9 * w_w && i_h > 0.9 * w_h
                    })
                {
                    traverse_elements(
                        &child,
                        &child_fp.frame.unwrap_or(*parent_frame),
                        window_frame,
                        cache,
                        target,
                        vis_level,
                        depth + 1,
                    );
                } else {
                    let roi = role_to_interest(&child_fp.role);
                    let context = match roi {
                        RoleOfInterest::TextField | RoleOfInterest::StaticText => {
                            Some(child.get_string_value_or_description().unwrap_or_default())
                        }
                        _ => None,
                    };
                    cache.force_add(
                        &child,
                        context,
                        roi,
                        child_fp.frame.and_then(|f| f.intersect(window_frame)),
                    );
                }
            }
        }

        return;
    }

    // If invisible, return early
    // NOTE: `parent_frame` should be monotonically decreasing,
    // and always included in `window_frame`
    let new_frame = match vis_level {
        VisibilityCheckingLevel::Loose | VisibilityCheckingLevel::Loosest => {
            let Some(new_frame) = ele_fp.visible_frame(window_frame) else {
                return;
            };
            new_frame
        }
        // Check intersection with parent frame
        _ => {
            let Some(new_frame) = ele_fp.visible_frame(parent_frame) else {
                return;
            };
            if vis_level == VisibilityCheckingLevel::Strict {
                new_frame
            } else {
                ele_fp
                    .frame
                    .and_then(|f| f.intersect(window_frame))
                    .unwrap_or(*parent_frame)
            }
        }
    };

    // Try matching custom target first
    if let Target::Custom(ct) = target
        && ele_fp.match_custom_target(ct)
        && element.match_custom_target(ct)
    {
        cache.add(element, None, RoleOfInterest::CustomTarget, ele_fp.frame);
    };

    let mut window_frame = *window_frame;

    #[allow(non_upper_case_globals)]
    match ele_fp.role.as_str() {
        // TODO: DOM Class List based image searching for icon button
        kAXPopUpButtonRole | kAXButtonRole | "AXRadioButton" => match target {
            Target::Clickable => {
                cache.add(element, None, RoleOfInterest::Button, ele_fp.frame);
            }
            Target::Text => {
                if let Ok(ctx) = element
                    .label_value()
                    .or_else(|_| element.title())
                    .or_else(|_| element.description())
                    .map(|cf| cf.to_string())
                {
                    cache.add(element, Some(ctx), RoleOfInterest::Button, ele_fp.frame);
                }
            }
            _ => (),
        },
        kAXCellRole => {
            if *target == Target::Clickable {
                cache.add(element, None, RoleOfInterest::Cell, ele_fp.frame);
            }
        }
        // NOTE: first found in Discord app
        // hopefully won't cause too many false positives
        kAXRowRole => {
            if *target == Target::Clickable
                && !element.children().is_ok_and(|children| {
                    children
                        .iter()
                        .any(|c| c.role().is_ok_and(|r| r == kAXCellRole))
                })
            {
                cache.add(element, None, RoleOfInterest::Cell, ele_fp.frame);
            }
        }
        kAXImageRole => match target {
            Target::Image | Target::ImageOCR => {
                cache.add(element, None, RoleOfInterest::Image, ele_fp.frame);
            }
            Target::Clickable if element.is_clickable() => {
                cache.add(element, None, RoleOfInterest::Button, ele_fp.frame);
            }
            _ => (),
        },
        kAXStaticTextRole => match target {
            Target::Clickable if element.is_clickable() => {
                cache.add(element, None, RoleOfInterest::Button, ele_fp.frame);
            }
            Target::Text => {
                if let Some(value) = element.get_string_value_or_description() {
                    cache.add(
                        element,
                        Some(value),
                        RoleOfInterest::StaticText,
                        ele_fp.frame,
                    );
                }
            }
            _ => (),
        },
        // NOTE: narrow down to window frame at "Window-ish" nodes.
        // This is useful for y axis scroll-off detection of electron apps
        kAXWindowRole | kAXScrollAreaRole | "AXWebArea"
            if vis_level != VisibilityCheckingLevel::Loosest =>
        {
            if let Some(area_frame) = ele_fp.frame.and_then(|f| f.intersect(&window_frame)) {
                window_frame = area_frame;
            };
        }
        // NOTE: don't do it for AXSectionList, e.g. Apple Music
        kAXListRole if element.subrole().is_ok_and(|r| r == kAXContentListSubrole) => {
            if let Some(area_frame) = ele_fp.frame.and_then(|f| f.intersect(&window_frame))
                && {
                    let (w, h) = area_frame.size();
                    // HACK: Some content lists may have fake sizes, e.g. Slack
                    w > 10.0 && h > 10.0
                }
            {
                window_frame = area_frame;
            };
        }
        kAXComboBoxRole | kAXTextFieldRole | kAXTextAreaRole => match target {
            Target::Editable => {
                cache.add(
                    element,
                    element.get_string_value_or_description(),
                    RoleOfInterest::TextField,
                    ele_fp.frame,
                );
            }
            Target::Text => {
                if let Some(value) = element.get_string_value_or_description()
                    && !value.is_empty()
                {
                    cache.add(
                        element,
                        Some(value),
                        RoleOfInterest::TextField,
                        ele_fp.frame,
                    );
                }
            }
            // NOTE: Even if not clickable, still could be focused on click
            Target::Clickable => {
                cache.add(element, None, RoleOfInterest::TextField, ele_fp.frame);
            }
            _ => (),
        },
        kAXCheckBoxRole => match target {
            Target::Clickable => {
                cache.add(element, None, RoleOfInterest::CheckBox, ele_fp.frame);
            }
            Target::Text => {
                if let Ok(value) = element.description().map(|v| v.to_string()) {
                    cache.add(
                        element,
                        Some(value),
                        RoleOfInterest::StaticText,
                        ele_fp.frame,
                    );
                }
            }
            _ => (),
        },
        "AXHeading" => {
            if *target == Target::Text
                && let Ok(value) = element
                    .description()
                    .or_else(|_| element.label_value())
                    .map(|v| v.to_string())
            {
                cache.add(
                    element,
                    Some(value),
                    RoleOfInterest::StaticText,
                    ele_fp.frame,
                );
            }
        }
        kAXGroupRole => match target {
            Target::Clickable if element.is_clickable() => {
                cache.add(element, None, RoleOfInterest::Button, ele_fp.frame);
            }
            // NOTE: Potential texts in leaf AXGroup
            Target::ImageOCR if !element.has_children() => {
                cache.add(element, None, RoleOfInterest::Image, ele_fp.frame);
            }
            _ => (),
        },
        kAXMenuItemRole => match target {
            Target::Text => {
                if let Some(title) = element.get_attribute_string(kAXTitleAttribute) {
                    cache.add(element, Some(title), RoleOfInterest::MenuItem, ele_fp.frame);
                }
            }
            Target::MenuItem | Target::Clickable => {
                cache.add(element, None, RoleOfInterest::MenuItem, ele_fp.frame);
            }
            _ => (),
        },
        kAXScrollBarRole => {
            if *target == Target::Scrollable {
                cache.add(element, None, RoleOfInterest::ScrollBar, ele_fp.frame);
            }
        }
        _ => match target {
            Target::Clickable if element.is_clickable() => {
                cache.add(element, None, RoleOfInterest::Button, ele_fp.frame);
            }
            _ => (),
        },
    }

    if let Ok(children) = element.children() {
        for child in &children {
            // NOTE: Some apps, like App Store, have circular referencing
            if *child == *element {
                continue;
            }
            traverse_elements(
                &child,
                &new_frame,
                &window_frame,
                cache,
                target,
                vis_level,
                depth + 1,
            );
        }
    }
}
