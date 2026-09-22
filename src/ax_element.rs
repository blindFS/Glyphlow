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
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::mpsc::Sender};

const BASIC_ATTRIBUTES: [&str; 4] = [
    kAXRoleAttribute,
    kAXPositionAttribute,
    kAXSizeAttribute,
    kAXHiddenAttribute,
];

thread_local! {
    /// [`BASIC_ATTRIBUTES`] as CF objects.
    ///
    /// Every visited element needs the same four attribute names, and one
    /// traversal visits thousands of them, so the names are built once per
    /// thread instead of once per call. `CFArray` is not `Send`, which rules out
    /// a plain `static`; each traversal thread builds its own copy.
    static BASIC_ATTRIBUTES_CF: CFArray<CFString> = CFArray::from_CFTypes(
        &BASIC_ATTRIBUTES
            .iter()
            .map(|&name| CFString::new(name))
            .collect::<Vec<_>>(),
    );

    /// The two attributes [`GetAttribute::get_frame`] reads, as CF objects.
    /// That runs once per ancestor step while walking up the tree, so it is
    /// cached for the same reason.
    static FRAME_ATTRIBUTES_CF: CFArray<CFString> = CFArray::from_CFTypes(&[
        CFString::new(kAXPositionAttribute),
        CFString::new(kAXSizeAttribute),
    ]);
}

pub enum ElementSignal {
    // Traversal
    ElementFound(Option<ElementOfInterest>),
    TraversalFinished(Target),
}

pub(crate) fn match_helper(pattern: &str, value: &impl ToString) -> bool {
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
        BASIC_ATTRIBUTES_CF.with(|cf_attributes| {
            let mut values_ref: CFArrayRef = std::ptr::null();
            let err = unsafe {
                AXUIElementCopyMultipleAttributeValues(
                    element.as_concrete_TypeRef(),
                    cf_attributes.as_concrete_TypeRef(),
                    // Don't stop on error
                    0,
                    &mut values_ref,
                )
            };

            if err != kAXErrorSuccess || values_ref.is_null() {
                return None;
            }

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
        })
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledTarget {
    pub role: String,
    pub subrole: Option<String>,
    #[serde(with = "serde_regex")]
    pub label: Option<Regex>,
    #[serde(with = "serde_regex")]
    pub value: Option<Regex>,
    #[serde(with = "serde_regex")]
    pub title: Option<Regex>,
    #[serde(with = "serde_regex")]
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
        element: &AXUIElement,
        context: Option<String>,
        role: RoleOfInterest,
        frame: Option<Frame>,
    ) -> Option<Self> {
        frame.map(|f| Self::new(element.clone(), context, role, f))
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

        let zero_frame = Frame::default();

        loop {
            if let Ok(parent) = other.parent() {
                *other = parent;
                if other == this {
                    return true;
                }
                // Early stop
                if other.get_frame(zero_frame).contains(&self.frame) {
                    return false;
                }
            } else {
                return false;
            }
        }
    }

    pub fn ascii_search_target(&self) -> String {
        self.context
            .as_ref()
            .map(|t| lower_ascii(t))
            .or_else(|| self.element().map(|e| lower_ascii(&e.search_target())))
            .unwrap_or_default()
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
        let choices: Vec<(&str, Frame, bool)> = self
            .cache
            .iter()
            .map(|eoi| {
                let is_valid = ref_role.is_none_or(|ref_role| *ref_role == eoi.role());
                (
                    eoi.context.as_deref().unwrap_or_default(),
                    eoi.frame,
                    is_valid,
                )
            })
            .collect();
        select_range_helper(&choices, idx1, idx2)
    }
}

fn role_to_interest(role: &str) -> RoleOfInterest {
    #[allow(non_upper_case_globals)]
    match role {
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
    fn same_sub_tree(&self, other: &AXUIElement, depth: u8) -> bool;
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
        let frame = FRAME_ATTRIBUTES_CF.with(|cf_attributes| {
            let mut values_ref: CFArrayRef = std::ptr::null();
            let err = unsafe {
                AXUIElementCopyMultipleAttributeValues(
                    self.as_concrete_TypeRef(),
                    cf_attributes.as_concrete_TypeRef(),
                    // Don't stop on error
                    0,
                    &mut values_ref,
                )
            };

            if err != kAXErrorSuccess || values_ref.is_null() {
                return None;
            }

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
        });
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

    fn same_sub_tree(&self, other: &AXUIElement, depth: u8) -> bool {
        if self == other {
            return true;
        }

        let mut trace_self = vec![self.clone()];
        let mut trace_other = vec![other.clone()];

        for _ in 0..depth {
            let mut has_new = false;
            if let Some(p_this) = trace_self.last().and_then(|e| e.parent().ok()) {
                if trace_other.contains(&p_this) {
                    return true;
                }
                trace_self.push(p_this);
                has_new = true;
            }
            if let Some(p_other) = trace_other.last().and_then(|e| e.parent().ok()) {
                if trace_self.contains(&p_other) {
                    return true;
                }
                trace_other.push(p_other);
                has_new = true;
            }

            if !has_new {
                break;
            }
        }
        false
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

#[derive(Debug, Default, PartialEq, Clone, Serialize, Deserialize)]
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
    result_tx: &Sender<ElementSignal>,
    depth: u8,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let element = &ts_elem.0;
    let Some(ele_fp) = ElementBasicAttributes::from(element) else {
        return;
    };

    // Every arm of the dispatch below reports an element the same way:
    // `ElementOfInterest` carries the role the caller is looking for, and
    // whatever text a search could match on. The macro keeps the role/target
    // table from being buried under a five-line `send` per arm; the
    // four-argument form is for the child walk, which reports a child it has
    // already read rather than the element currently being visited.
    macro_rules! found {
        ($role:expr) => {
            found!(element, ele_fp.frame, $role, None)
        };
        ($role:expr, $context:expr) => {
            found!(element, ele_fp.frame, $role, $context)
        };
        ($element:expr, $frame:expr, $role:expr, $context:expr) => {{
            let _ = result_tx.send(ElementSignal::ElementFound(ElementOfInterest::try_new(
                $element, $context, $role, $frame,
            )));
        }};
    }

    // PERF: Performance critical! Exclude electron elements scrolled off y axis,
    if ele_fp.frame.is_some_and(|f| {
        let (w, h) = f.size();
        (h == 0.0 && f.bottom_right.y == window_frame.bottom_right.y)
            || (h == 1.0 && f.top_left.y == window_frame.top_left.y
            // NOTE: 1. Keep full width elements, e.g. Brave google search
            && w != window_frame.size().0
            // NOTE: 2. Keep application dialog, e.g. GitHub issue dialog
            && element.subrole().ok().is_none_or(|subrole| subrole != "AXApplicationDialog"))
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
                        result_tx,
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
                    found!(
                        &child,
                        child_fp.frame.and_then(|f| f.intersect(window_frame)),
                        roi,
                        context
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
        found!(RoleOfInterest::CustomTarget);
    };

    let mut window_frame = *window_frame;

    #[allow(non_upper_case_globals)]
    match ele_fp.role.as_str() {
        // TODO: DOM Class List based image searching for icon button
        kAXPopUpButtonRole | kAXButtonRole | "AXRadioButton" => match target {
            Target::Clickable => found!(RoleOfInterest::Button),
            Target::Text => {
                if let Ok(ctx) = element
                    .label_value()
                    .or_else(|_| element.title())
                    .or_else(|_| element.description())
                    .map(|cf| cf.to_string())
                {
                    found!(RoleOfInterest::Button, Some(ctx));
                }
            }
            _ => (),
        },
        kAXCellRole => {
            if *target == Target::Clickable {
                found!(RoleOfInterest::Cell);
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
                found!(RoleOfInterest::Cell);
            }
        }
        kAXImageRole => match target {
            Target::Image | Target::ImageOCR => found!(RoleOfInterest::Image),
            Target::Clickable if element.is_clickable() => found!(RoleOfInterest::Button),
            _ => (),
        },
        "AXLink" => match target {
            Target::Text if !element.has_children() => found!(
                RoleOfInterest::StaticText,
                element
                    .title()
                    .or_else(|_| element.description())
                    .map(|cs| cs.to_string())
                    .ok()
            ),
            Target::Clickable if element.is_clickable() => found!(RoleOfInterest::StaticText),
            _ => (),
        },
        kAXStaticTextRole => match target {
            Target::Clickable if element.is_clickable() => found!(RoleOfInterest::Button),
            Target::Text => {
                if let Some(value) = element.get_string_value_or_description() {
                    found!(RoleOfInterest::StaticText, Some(value));
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
                found!(
                    RoleOfInterest::TextField,
                    element.get_string_value_or_description()
                );
            }
            Target::Text => {
                if let Some(value) = element.get_string_value_or_description()
                    && !value.is_empty()
                {
                    found!(RoleOfInterest::TextField, Some(value));
                }
            }
            // NOTE: Even if not clickable, still could be focused on click
            Target::Clickable => found!(RoleOfInterest::TextField),
            _ => (),
        },
        kAXCheckBoxRole => match target {
            Target::Clickable => found!(RoleOfInterest::CheckBox),
            Target::Text => {
                if let Ok(value) = element.description().map(|v| v.to_string()) {
                    found!(RoleOfInterest::CheckBox, Some(value));
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
                found!(RoleOfInterest::StaticText, Some(value));
            }
        }
        kAXGroupRole => match target {
            Target::Clickable if element.is_clickable() => found!(RoleOfInterest::Button),
            // NOTE: Potential texts in leaf AXGroup
            Target::ImageOCR if !element.has_children() => found!(RoleOfInterest::Image),
            _ => (),
        },
        kAXMenuItemRole => match target {
            Target::Text => {
                if let Some(title) = element.get_attribute_string(kAXTitleAttribute) {
                    found!(RoleOfInterest::MenuItem, Some(title));
                }
            }
            Target::Clickable => found!(RoleOfInterest::MenuItem),
            _ => (),
        },
        kAXScrollBarRole => {
            if *target == Target::Scrollable {
                found!(RoleOfInterest::ScrollBar);
            }
        }
        _ => match target {
            Target::Clickable if element.is_clickable() => found!(RoleOfInterest::Button),
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

            traverse_elements(
                safe_child,
                &new_frame,
                &window_frame,
                target,
                vis_level,
                result_tx,
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
    result_tx: &Sender<ElementSignal>,
) {
    autoreleasepool(|_| {
        traverse_elements(
            root,
            &parent_frame,
            &window_frame,
            &target,
            vis_level,
            result_tx,
            0,
        );
        let _ = result_tx.send(ElementSignal::TraversalFinished(target));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CustomTarget;
    use rstest::rstest;

    /// Build a minimal `CustomTarget` with only the `role` field set.
    fn ct_role(role: &str) -> CustomTarget {
        CustomTarget {
            role: role.into(),
            ..Default::default()
        }
    }

    /// Build a `CustomTarget` constrained to an exact element size.
    fn ct_size(role: &str, width: f64, height: f64) -> CustomTarget {
        CustomTarget {
            role: role.into(),
            size: Some((width, height)),
            ..Default::default()
        }
    }

    /// `match_helper` is a case-insensitive substring test, with `|` separating
    /// alternatives. An empty pattern is contained in everything, itself
    /// included.
    #[rstest]
    #[case::exact_lowercase("Button", "button", true)]
    #[case::pattern_lowercase("BUTTON", "AXButton", true)]
    #[case::both_uppercase("button", "AXBUTTON", true)]
    #[case::substring("menu", "AXMenuItem", true)]
    #[case::empty_pattern_matches_anything("", "anything", true)]
    #[case::empty_pattern_matches_empty("", "", true)]
    #[case::mismatch("image", "AXButton", false)]
    #[case::pipe_first_alternative("button|textfield", "AXButton", true)]
    #[case::pipe_second_alternative("button|textfield", "AXTextField", true)]
    #[case::pipe_tolerates_spaces("button | textfield", "AXTextField", true)]
    fn match_helper_is_case_insensitive_substring_with_pipe_alternatives(
        #[case] pattern: &str,
        #[case] value: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(match_helper(pattern, &value), expected);
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

    /// `action` names an accessibility action to perform, not a pattern, so a
    /// `|` in it must survive verbatim instead of being read as alternatives.
    #[test]
    fn action_is_kept_verbatim_and_not_split_on_pipe() {
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

    /// A target matches only when *every* field it constrains matches. Fields
    /// left unset are ignored, and an element with no frame can never satisfy a
    /// size constraint.
    #[rstest]
    #[case::role_exact("AXButton", None, ct_role("AXButton"), true)]
    #[case::role_substring_is_case_insensitive("AXButton", None, ct_role("button"), true)]
    #[case::role_pipe_is_or("AXMenuItem", None, ct_role("button|menuitem"), true)]
    #[case::role_mismatch("AXImage", None, ct_role("button"), false)]
    #[case::size_matches(
        "AXButton",
        Some((0.0, 0.0, 100.0, 50.0)),
        ct_size("AXButton", 100.0, 50.0),
        true
    )]
    #[case::size_mismatch(
        "AXButton",
        Some((0.0, 0.0, 200.0, 80.0)),
        ct_size("AXButton", 100.0, 50.0),
        false
    )]
    #[case::size_required_but_element_has_no_frame(
        "AXButton",
        None,
        ct_size("AXButton", 100.0, 50.0),
        false
    )]
    fn basic_attributes_match_only_when_every_constraint_holds(
        #[case] role: &str,
        #[case] frame: Option<(f64, f64, f64, f64)>,
        #[case] target: CustomTarget,
        #[case] expected: bool,
    ) {
        let elem = make_basic(
            role,
            frame.map(|(x1, y1, x2, y2)| Frame::new(x1, y1, x2, y2)),
        );
        let target = CompiledTarget::new(&target).expect("test targets must compile");

        assert_eq!(elem.match_custom_target(&target), expected);
    }

    /// The AX role string is all the accessibility tree gives us to decide what
    /// an element *is*, and everything downstream keys off the result: which
    /// hint box is drawn, which menu opens, and whether a workflow applies. An
    /// unrecognised role must fall back to `Generic` rather than to something
    /// more specific, or we would claim a button is a text field.
    #[rstest]
    #[case::image(kAXImageRole, RoleOfInterest::Image)]
    #[case::text_field(kAXTextFieldRole, RoleOfInterest::TextField)]
    #[case::text_area(kAXTextAreaRole, RoleOfInterest::TextField)]
    #[case::combo_box(kAXComboBoxRole, RoleOfInterest::TextField)]
    #[case::menu_item(kAXMenuItemRole, RoleOfInterest::MenuItem)]
    #[case::pop_up_button(kAXPopUpButtonRole, RoleOfInterest::Button)]
    #[case::button(kAXButtonRole, RoleOfInterest::Button)]
    #[case::check_box(kAXCheckBoxRole, RoleOfInterest::CheckBox)]
    #[case::static_text(kAXStaticTextRole, RoleOfInterest::StaticText)]
    #[case::scroll_bar(kAXScrollBarRole, RoleOfInterest::ScrollBar)]
    // These two have no `kAX*` constant: AppKit reports them by literal.
    #[case::radio_button("AXRadioButton", RoleOfInterest::Button)]
    #[case::heading("AXHeading", RoleOfInterest::StaticText)]
    // Roles we deliberately do not treat as anything special.
    #[case::group(kAXGroupRole, RoleOfInterest::Generic)]
    #[case::row(kAXRowRole, RoleOfInterest::Generic)]
    #[case::unknown_role("AXSomethingNew", RoleOfInterest::Generic)]
    #[case::empty_role("", RoleOfInterest::Generic)]
    fn maps_an_accessibility_role_to_our_own(
        #[case] ax_role: &str,
        #[case] expected: RoleOfInterest,
    ) {
        assert_eq!(role_to_interest(ax_role), expected);
    }

    /// Three text runs on one line, 45px apart — the shape a sentence's hint
    /// boxes produce. Pseudo elements are enough here because `select_range`
    /// only reads the context, the frame and the role.
    fn cache_of(words: &[&str]) -> ElementCache {
        let mut cache = ElementCache::default();
        for (i, word) in words.iter().enumerate() {
            let x = i as f64 * 45.0;
            cache.cache.push(ElementOfInterest::pseudo(
                Some((*word).into()),
                Frame::new(x, 0.0, x + 40.0, 10.0),
            ));
        }
        cache
    }

    /// `select_range` turns the two picked hints into the text that gets copied.
    /// Without a role reference every element between the two ends is fair game.
    #[test]
    fn a_range_without_a_role_reference_spans_every_element() {
        let cache = cache_of(&["alpha ", "beta ", "gamma"]);

        let (text, frame) = cache.select_range(0, 2, None).expect("range is selectable");

        assert_eq!(text, "alpha beta gamma");
        assert_eq!(frame, Frame::new(0.0, 0.0, 130.0, 10.0));
    }

    /// A role reference restricts the range to elements of that role. When
    /// nothing in between matches, the range is refused outright instead of
    /// falling back to "ignore the filter" — otherwise a multi-selection across
    /// two text fields would silently swallow the button sitting between them.
    #[test]
    fn a_role_reference_restricts_the_range_or_refuses_it() {
        let cache = cache_of(&["alpha ", "beta ", "gamma"]);
        let own_role = cache.cache[0].role();

        assert!(
            cache.select_range(0, 2, Some(&own_role)).is_some(),
            "every element here has the same role, so the range survives"
        );

        assert_eq!(
            cache.select_range(0, 2, Some(&RoleOfInterest::TextField)),
            None,
            "no element matches that role, so there is nothing to select"
        );
    }

    /// The element explorer walks the raw tree, so it bypasses every size and
    /// content filter and keeps duplicates — the user asked to see all of it.
    /// It also always reports the index it just added.
    #[test]
    fn the_element_explorer_force_adds_anything() {
        let mut cache = ElementCache::new(10.0, 10.0, 20.0);
        // Zero-sized and with no context: `add` would reject this outright.
        let ele = ElementOfInterest::pseudo(None, Frame::new(0.0, 0.0, 0.0, 0.0));

        assert_eq!(
            cache.add_by_target(ele.clone(), &Target::ChildElement),
            Some(0)
        );
        assert_eq!(cache.cache.len(), 1);

        assert_eq!(
            cache.add_by_target(ele, &Target::ChildElement),
            Some(1),
            "the reported index must follow the cache"
        );
        assert_eq!(cache.cache.len(), 2, "the explorer keeps duplicates");
    }

    /// Outside the explorer a pseudo element is never cached. A clipboard-backed
    /// selection has no accessibility element, so there is nothing to draw a
    /// hint box on and nothing to press later.
    #[test]
    fn a_pseudo_element_is_never_cached_outside_the_element_explorer() {
        let mut cache = ElementCache::new(0.0, 0.0, 0.0);
        let ele =
            ElementOfInterest::pseudo(Some("clipboard".into()), Frame::new(0.0, 0.0, 100.0, 20.0));

        assert_eq!(cache.add_by_target(ele, &Target::Text), None);
        assert!(cache.cache.is_empty());
    }

    /// `clear` has to forget the de-duplication map as well, or the next
    /// activation would overwrite a stale entry instead of adding a fresh one.
    #[test]
    fn clear_forgets_the_de_duplication_map() {
        let mut cache = ElementCache::new(0.0, 0.0, 0.0);
        cache.add_by_target(
            ElementOfInterest::pseudo(None, Frame::new(0.0, 0.0, 10.0, 10.0)),
            &Target::ChildElement,
        );

        cache.clear();

        assert!(cache.cache.is_empty());
        assert!(cache.seen_center.is_empty());
    }

    /// A pseudo element carries its searchable text in `context` because it has no
    /// accessibility element to read from. The folding itself is `lower_ascii`'s
    /// business (covered in `util`), so all this pins is that the context is used
    /// and that a missing context yields an empty target rather than a panic.
    #[rstest]
    #[case::lower_cased(Some("Mixed Case"), "mixed case")]
    #[case::no_context(None, "")]
    fn a_pseudo_element_searches_its_context(
        #[case] context: Option<&str>,
        #[case] expected: &str,
    ) {
        let element = ElementOfInterest::pseudo(
            context.map(str::to_string),
            Frame::new(0.0, 0.0, 10.0, 10.0),
        );
        assert_eq!(element.ascii_search_target(), expected);
    }
}
