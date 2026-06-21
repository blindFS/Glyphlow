use crate::{
    config::{CustomTarget, GlyphlowConfig, RoleOfInterest, VisibilityCheckingLevel},
    util::{Frame, lower_ascii, select_range_helper},
};
use accessibility::{AXAttribute, AXUIElement, AXUIElementAttributes};
use accessibility_sys::{
    AXUIElementCopyMultipleAttributeValues, AXValueCreate, AXValueGetValue, AXValueRef,
    kAXButtonRole, kAXCellRole, kAXCheckBoxRole, kAXComboBoxRole, kAXErrorSuccess, kAXGroupRole,
    kAXHiddenAttribute, kAXImageRole, kAXMenuItemRole, kAXPopUpButtonRole, kAXPositionAttribute,
    kAXPressAction, kAXRoleAttribute, kAXRowRole, kAXScrollAreaRole, kAXScrollBarRole,
    kAXSelectedTextRangeAttribute, kAXSizeAttribute, kAXStaticTextRole, kAXTextAreaRole,
    kAXTextFieldRole, kAXTitleAttribute, kAXValueTypeCFRange, kAXValueTypeCGPoint,
    kAXValueTypeCGSize, kAXWindowRole,
};
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFRange, CFType, CFTypeRef, FromVoid, TCFType},
    boolean::CFBoolean,
    string::CFString,
};
use objc2::rc::autoreleasepool;
use objc2_core_foundation::{CGPoint, CGSize};
use regex::Regex;
use std::{collections::HashMap, sync::mpsc::Sender};

const BASIC_ATTRIBUTES: [&str; 4] = [
    kAXRoleAttribute,
    kAXPositionAttribute,
    kAXSizeAttribute,
    kAXHiddenAttribute,
];

pub enum ElementSignal {
    // Traversal
    ElementFound(Option<ElementOfInterest>),
    TraversalFinished(Target),
}

fn match_helper(pattern: &str, value: &impl ToString) -> bool {
    let value = value.to_string().to_lowercase();
    pattern
        .to_lowercase()
        .split('|')
        .any(|t| value.contains(t.trim()))
}

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

    fn match_custom_target(&self, target: &CompiledTarget) -> bool {
        if let Some(size) = target.size
            && !self.frame.is_some_and(|f| f.size() == size)
        {
            return false;
        }
        match_helper(&target.role, &self.role)
    }
}

/// A [`CustomTarget`] with string fields pre-compiled into [`Regex`] objects.
/// Build once per workflow search action; reuse across the entire element traversal.
#[derive(Debug, Clone)]
pub struct CompiledTarget {
    pub role: String,
    pub subrole: Option<String>,
    pub label: Option<Regex>,
    pub value: Option<Regex>,
    pub title: Option<Regex>,
    pub description: Option<Regex>,
    pub size: Option<(f64, f64)>,
    pub action: Option<String>,
}

impl PartialEq for CompiledTarget {
    fn eq(&self, other: &Self) -> bool {
        let opt_re_eq = |a: &Option<Regex>, b: &Option<Regex>| match (a, b) {
            (Some(x), Some(y)) => x.as_str() == y.as_str(),
            (None, None) => true,
            _ => false,
        };
        self.role == other.role
            && self.subrole == other.subrole
            && opt_re_eq(&self.label, &other.label)
            && opt_re_eq(&self.value, &other.value)
            && opt_re_eq(&self.title, &other.title)
            && opt_re_eq(&self.description, &other.description)
            && self.size == other.size
            && self.action == other.action
    }
}

impl CompiledTarget {
    pub fn new(ct: &CustomTarget) -> Result<Self, regex::Error> {
        let compile_opt = |opt: &Option<String>| opt.as_deref().map(Regex::new).transpose();
        Ok(Self {
            role: ct.role.to_owned(),
            subrole: ct.subrole.to_owned(),
            label: compile_opt(&ct.label)?,
            value: compile_opt(&ct.value)?,
            title: compile_opt(&ct.title)?,
            description: compile_opt(&ct.description)?,
            size: ct.size,
            action: ct.action.to_owned(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThreadSafeElement(pub AXUIElement);
unsafe impl Send for ThreadSafeElement {}

#[derive(Clone, Debug, PartialEq)]
pub enum ElementKind {
    Standard {
        element: ThreadSafeElement,
        role: RoleOfInterest,
    },
    Pseudo,
}

#[derive(Clone, Debug, PartialEq)]
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
            kind: ElementKind::Standard {
                element: ThreadSafeElement(element),
                role,
            },
            context,
            frame,
        }
    }

    pub fn try_new(
        element: AXUIElement,
        context: Option<String>,
        role: RoleOfInterest,
        frame: Option<Frame>,
    ) -> Option<Self> {
        frame.map(|f| Self::new(element, context, role, f))
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
            ElementKind::Standard { element, .. } => Some(&element.0),
            ElementKind::Pseudo => None,
        }
    }

    pub fn equals_element(&self, other: &AXUIElement) -> bool {
        self.element().is_some_and(|this| this == other)
    }

    pub fn is_ancestor_of(&self, other: &mut AXUIElement) -> bool {
        let Some(this) = self.element() else {
            return false;
        };

        loop {
            if other == this {
                return true;
            }
            if let Ok(parent) = other.parent() {
                *other = parent;
            } else {
                return false;
            }
        }
    }

    pub fn ascii_search_target(&self) -> String {
        let raw = self
            .context
            .clone()
            .or_else(|| self.element().map(|e| e.search_target()))
            .unwrap_or_default();
        lower_ascii(&raw)
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

    pub fn add_by_target(&mut self, ele: ElementOfInterest, target: &Target) -> Option<usize> {
        let idx = self.cache.len();
        if *target == Target::ChildElement {
            self.force_add(ele);
            Some(idx)
        } else {
            self.add(ele)
        }
    }

    fn force_add(&mut self, eoi: ElementOfInterest) {
        let ElementOfInterest { frame, .. } = &eoi;
        let (x, y) = frame.center();
        // f64 to u64 for hashing
        let center = (x.to_bits(), y.to_bits());

        self.seen_center.insert(center, self.cache.len());
        self.cache.push(eoi);
    }

    fn add(&mut self, eoi: ElementOfInterest) -> Option<usize> {
        let ElementOfInterest {
            kind: ElementKind::Standard { element, role },
            context,
            frame,
        } = &eoi
        else {
            return None;
        };

        // NOTE: Use parent frames for scroll bars,
        // replaces the existing AXScrollArea in later center point check
        let frame = if *role == RoleOfInterest::ScrollBar
            && let Some(parent_frame) = element
                .0
                .parent()
                .ok()
                .and_then(|p| ElementBasicAttributes::from(&p))
                .and_then(|p_fp| p_fp.frame)
        {
            parent_frame
        } else {
            *frame
        };

        let (w, h) = frame.size();
        match role {
            // NOTE: some roles to keep
            RoleOfInterest::Generic | RoleOfInterest::ScrollBar | RoleOfInterest::CheckBox => {}
            // HACK: some menu items (like Apple Intelligence writing tools)
            // may have zero sized shadows, skip them to keep the workflow going
            RoleOfInterest::CustomTarget if w != 0.0 && h != 0.0 => {}
            RoleOfInterest::Image if w.min(h) < self.image_min_size => {
                return None;
            }
            // Keep large enough images
            RoleOfInterest::Image => (),
            // Keep large enough text fields even if the text can be empty
            RoleOfInterest::TextField
                if (w < self.element_min_width || h < self.element_min_height) =>
            {
                return None;
            }
            RoleOfInterest::TextField => (),
            // Check text before size, keep small texts
            _ if context.is_some()
                // Skip elements with empty/nonsense text
                && context.as_ref().is_some_and(|ctx| {
                    ctx.is_empty()
                        || ctx
                            .chars()
                            .all(|c| c.is_ascii_punctuation() || c.is_whitespace())
                }) =>
            {
                return None;
            }
            _ if (w < self.element_min_width || h < self.element_min_height) => {
                return None;
            }
            _ => (),
        }

        let (x, y) = frame.center();
        // f64 to u64 for hashing
        let center = (x.to_bits(), y.to_bits());

        // NOTE: de-duplication for DOM elements
        if let Some(idx) = self.seen_center.get(&center) {
            self.cache[*idx] = eoi;
            None
        } else {
            self.seen_center.insert(center, self.cache.len());
            let mut eoi = eoi;
            eoi.frame = frame;
            self.cache.push(eoi);
            Some(self.cache.len() - 1)
        }
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
    fn search_target(&self) -> String;
    fn is_clickable(&self) -> bool;
    fn has_children(&self) -> bool;
    fn match_custom_target(&self, target: &CompiledTarget) -> bool;
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
    }

    fn search_target(&self) -> String {
        let mut msg = String::new();

        if let Ok(t) = self.title() {
            msg.push_str(&format!("{t} "));
        }

        if let Ok(l) = self.label_value() {
            msg.push_str(&format!("{l} "));
        }

        if let Ok(d) = self.description() {
            msg.push_str(&format!("{d} "));
        }

        if let Some(v) = self.value().ok().and_then(|v| v.downcast::<CFString>()) {
            msg.push_str(&format!("{v}"));
        }

        msg
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

    fn match_custom_target(&self, target: &CompiledTarget) -> bool {
        if let Some(sr) = target.subrole.as_ref()
            && !self.subrole().is_ok_and(|s| match_helper(sr, &s))
        {
            return false;
        }
        if let Some(re) = target.description.as_ref()
            && !self
                .description()
                .is_ok_and(|d| re.is_match(&d.to_string()))
        {
            return false;
        }
        if let Some(re) = target.title.as_ref()
            && !self.title().is_ok_and(|t| re.is_match(&t.to_string()))
        {
            return false;
        }
        if let Some(re) = target.label.as_ref()
            && !self
                .label_value()
                .is_ok_and(|l| re.is_match(&l.to_string()))
        {
            return false;
        }
        if let Some(re) = target.value.as_ref()
            && !self
                .value()
                .ok()
                .and_then(|v| v.downcast::<CFString>())
                .is_some_and(|v| re.is_match(&v.to_string()))
        {
            return false;
        }
        if let Some(a) = target.action.as_ref()
            && !self
                .action_names()
                .is_ok_and(|names| names.iter().any(|n| match_helper(a, &*n)))
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
    ChildElement,
    Scrollable,
    Custom(Box<CompiledTarget>),
}

const MAX_DEPTH: u8 = 200;

fn traverse_elements(
    ts_elem: ThreadSafeElement,
    parent_frame: &Frame,
    window_frame: &Frame,
    target: &Target,
    vis_level: VisibilityCheckingLevel,
    result_tx: Sender<ElementSignal>,
    depth: u8,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let element = &ts_elem.0;
    let Some(ele_fp) = ElementBasicAttributes::from(element) else {
        return;
    };

    // PERF: Performance critical! Exclude electron elements scrolled off y axis,
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
                        ThreadSafeElement(child.to_owned()),
                        &child_fp.frame.unwrap_or(*parent_frame),
                        window_frame,
                        target,
                        vis_level,
                        result_tx.clone(),
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
                    let _ =
                        result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                            child.to_owned(),
                            context,
                            roi,
                            child_fp.frame.and_then(|f| f.intersect(window_frame)),
                        )));
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
        let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
            element.clone(),
            None,
            RoleOfInterest::CustomTarget,
            ele_fp.frame,
        )));
    };

    let mut window_frame = *window_frame;

    #[allow(non_upper_case_globals)]
    match ele_fp.role.as_str() {
        // TODO: DOM Class List based image searching for icon button
        kAXPopUpButtonRole | kAXButtonRole | "AXRadioButton" => match target {
            Target::Clickable => {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::Button,
                    ele_fp.frame,
                )));
            }
            Target::Text => {
                if let Ok(ctx) = element
                    .label_value()
                    .or_else(|_| element.title())
                    .or_else(|_| element.description())
                    .map(|cf| cf.to_string())
                {
                    let _ =
                        result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                            element.clone(),
                            Some(ctx),
                            RoleOfInterest::Button,
                            ele_fp.frame,
                        )));
                }
            }
            _ => (),
        },
        kAXCellRole => {
            if *target == Target::Clickable {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::Cell,
                    ele_fp.frame,
                )));
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
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::Cell,
                    ele_fp.frame,
                )));
            }
        }
        kAXImageRole => match target {
            Target::Image | Target::ImageOCR => {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::Image,
                    ele_fp.frame,
                )));
            }
            Target::Clickable if element.is_clickable() => {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::Button,
                    ele_fp.frame,
                )));
            }
            _ => (),
        },
        kAXStaticTextRole => match target {
            Target::Clickable if element.is_clickable() => {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::Button,
                    ele_fp.frame,
                )));
            }
            Target::Text => {
                if let Some(value) = element.get_string_value_or_description() {
                    let _ =
                        result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                            element.clone(),
                            Some(value),
                            RoleOfInterest::StaticText,
                            ele_fp.frame,
                        )));
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
        kAXComboBoxRole | kAXTextFieldRole | kAXTextAreaRole => match target {
            Target::Editable => {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    element.get_string_value_or_description(),
                    RoleOfInterest::TextField,
                    ele_fp.frame,
                )));
            }
            Target::Text => {
                if let Some(value) = element.get_string_value_or_description()
                    && !value.is_empty()
                {
                    let _ =
                        result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                            element.clone(),
                            Some(value),
                            RoleOfInterest::TextField,
                            ele_fp.frame,
                        )));
                }
            }
            // NOTE: Even if not clickable, still could be focused on click
            Target::Clickable => {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::TextField,
                    ele_fp.frame,
                )));
            }
            _ => (),
        },
        kAXCheckBoxRole => match target {
            Target::Clickable => {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::CheckBox,
                    ele_fp.frame,
                )));
            }
            Target::Text => {
                if let Ok(value) = element.description().map(|v| v.to_string()) {
                    let _ =
                        result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                            element.clone(),
                            Some(value),
                            RoleOfInterest::CheckBox,
                            ele_fp.frame,
                        )));
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
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    Some(value),
                    RoleOfInterest::StaticText,
                    ele_fp.frame,
                )));
            }
        }
        kAXGroupRole => match target {
            Target::Clickable if element.is_clickable() => {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::Button,
                    ele_fp.frame,
                )));
            }
            // NOTE: Potential texts in leaf AXGroup
            Target::ImageOCR if !element.has_children() => {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::Image,
                    ele_fp.frame,
                )));
            }
            _ => (),
        },
        kAXMenuItemRole => match target {
            Target::Text => {
                if let Some(title) = element.get_attribute_string(kAXTitleAttribute) {
                    let _ =
                        result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                            element.clone(),
                            Some(title),
                            RoleOfInterest::MenuItem,
                            ele_fp.frame,
                        )));
                }
            }
            Target::Clickable => {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::MenuItem,
                    ele_fp.frame,
                )));
            }
            _ => (),
        },
        kAXScrollBarRole => {
            if *target == Target::Scrollable {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::ScrollBar,
                    ele_fp.frame,
                )));
            }
        }
        _ => match target {
            Target::Clickable if element.is_clickable() => {
                let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                    element.clone(),
                    None,
                    RoleOfInterest::Button,
                    ele_fp.frame,
                )));
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
            let safe_child = ThreadSafeElement(child.to_owned());
            let tx_clone = result_tx.clone();

            traverse_elements(
                safe_child,
                &new_frame,
                &window_frame,
                target,
                vis_level,
                tx_clone,
                depth + 1,
            );
        }
    }
}

pub fn traverse(
    root: ThreadSafeElement,
    parent_frame: Frame,
    window_frame: Frame,
    target: Target,
    vis_level: VisibilityCheckingLevel,
    result_tx: Sender<ElementSignal>,
) {
    autoreleasepool(|_| {
        let target_c = target.clone();
        let tx_c = result_tx.clone();
        traverse_elements(
            root,
            &parent_frame,
            &window_frame,
            &target_c,
            vis_level,
            tx_c,
            0,
        );
        let _ = result_tx.send(ElementSignal::TraversalFinished(target));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CustomTarget;

    /// Build a minimal `CustomTarget` with only the `role` field set.
    fn ct_role(role: &str) -> CustomTarget {
        CustomTarget {
            role: role.into(),
            ..Default::default()
        }
    }

    #[test]
    fn match_helper_test() {
        assert!(match_helper("Button", &"button"));
        assert!(match_helper("BUTTON", &"AXButton"));
        assert!(match_helper("button", &"AXBUTTON"));
        assert!(match_helper("menu", &"AXMenuItem"));
        // An empty string is always contained in any string (including itself).
        assert!(match_helper("", &"anything"));
        assert!(match_helper("", &""));
        assert!(!match_helper("image", &"AXButton"));
    }

    #[test]
    fn match_helper_pipe() {
        assert!(match_helper("button|textfield", &"AXButton"));
        assert!(match_helper("button|textfield", &"AXTextField"));
        assert!(match_helper("button | textfield", &"AXTextField"));
    }

    #[test]
    fn compiled_target_new_valid() {
        let ct = CustomTarget {
            role: "MenuItem".into(),
            subrole: Some("AXContentList".into()),
            label: Some(r"Save.*".into()),
            value: Some(r"\d+".into()),
            title: Some("File".into()),
            description: Some("desc".into()),
            size: Some((100.0, 50.0)),
            action: Some("AXPress".into()),
        };

        let compiled = CompiledTarget::new(&ct).expect("Valid regexes should compile");

        assert_eq!(compiled.role, "MenuItem");
        assert_eq!(compiled.subrole.as_deref(), Some("AXContentList"));
        assert!(compiled.label.is_some());
        assert!(compiled.value.is_some());
        assert!(compiled.title.is_some());
        assert!(compiled.description.is_some());
        assert_eq!(compiled.size, Some((100.0, 50.0)));
        assert_eq!(compiled.action.as_deref(), Some("AXPress"));
    }

    #[test]
    fn compiled_target_new_none_fields() {
        let ct = ct_role("AXButton");
        let compiled = CompiledTarget::new(&ct).expect("Should compile with all-None optionals");

        assert_eq!(compiled.role, "AXButton");
        assert!(compiled.subrole.is_none());
        assert!(compiled.label.is_none());
        assert!(compiled.value.is_none());
        assert!(compiled.title.is_none());
        assert!(compiled.description.is_none());
        assert!(compiled.size.is_none());
        assert!(compiled.action.is_none());
    }

    #[test]
    fn compiled_target_new_invalid_regex_returns_err() {
        // An unclosed bracket is an invalid regex pattern.
        let ct = CustomTarget {
            role: "Button".into(),
            title: Some("[invalid".into()),
            ..Default::default()
        };

        assert!(
            CompiledTarget::new(&ct).is_err(),
            "Invalid regex should produce an Err"
        );
    }

    #[test]
    fn compiled_target_new_action_field_propagated() {
        let ct = CustomTarget {
            role: "MenuItem".into(),
            action: Some("press | highlight".into()),
            ..Default::default()
        };

        let compiled = CompiledTarget::new(&ct).unwrap();
        assert_eq!(compiled.action.as_deref(), Some("press | highlight"));
    }

    fn make_basic(role: &str, frame: Option<Frame>) -> ElementBasicAttributes {
        ElementBasicAttributes {
            role: role.into(),
            frame,
            hidden: false,
        }
    }

    #[test]
    fn basic_match_role_exact() {
        let elem = make_basic("AXButton", None);
        let target = CompiledTarget::new(&ct_role("AXButton")).unwrap();
        assert!(elem.match_custom_target(&target));
    }

    #[test]
    fn basic_match_role_substring() {
        // "button" is a substring of "AXButton" (case-insensitive).
        let elem = make_basic("AXButton", None);
        let target = CompiledTarget::new(&ct_role("button")).unwrap();
        assert!(elem.match_custom_target(&target));
    }

    #[test]
    fn basic_match_role_pipe_or() {
        let elem = make_basic("AXMenuItem", None);
        let target = CompiledTarget::new(&ct_role("button|menuitem")).unwrap();
        assert!(elem.match_custom_target(&target));
    }

    #[test]
    fn basic_match_role_mismatch() {
        let elem = make_basic("AXImage", None);
        let target = CompiledTarget::new(&ct_role("button")).unwrap();
        assert!(!elem.match_custom_target(&target));
    }

    #[test]
    fn basic_match_size_matches() {
        let frame = Frame::new(0.0, 0.0, 100.0, 50.0);
        let elem = make_basic("AXButton", Some(frame));
        let ct = CustomTarget {
            role: "AXButton".into(),
            size: Some((100.0, 50.0)),
            ..Default::default()
        };
        let target = CompiledTarget::new(&ct).unwrap();
        assert!(elem.match_custom_target(&target));
    }

    #[test]
    fn basic_match_size_mismatch() {
        let frame = Frame::new(0.0, 0.0, 200.0, 80.0);
        let elem = make_basic("AXButton", Some(frame));
        let ct = CustomTarget {
            role: "AXButton".into(),
            size: Some((100.0, 50.0)),
            ..Default::default()
        };
        let target = CompiledTarget::new(&ct).unwrap();
        assert!(!elem.match_custom_target(&target));
    }

    #[test]
    fn basic_match_size_required_but_no_frame() {
        // If the target requires a specific size but the element has no frame,
        // it should not match.
        let elem = make_basic("AXButton", None);
        let ct = CustomTarget {
            role: "AXButton".into(),
            size: Some((100.0, 50.0)),
            ..Default::default()
        };
        let target = CompiledTarget::new(&ct).unwrap();
        assert!(!elem.match_custom_target(&target));
    }

    #[test]
    fn basic_match_no_size_constraint_ignores_frame() {
        // When the target has no size constraint the element frame is irrelevant.
        let elem_no_frame = make_basic("AXButton", None);
        let target = CompiledTarget::new(&ct_role("AXButton")).unwrap();
        assert!(elem_no_frame.match_custom_target(&target));
    }
}
